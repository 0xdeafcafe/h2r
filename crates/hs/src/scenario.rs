use crate::{Catalog, Datum, Global, Node, Reference, Result, Script, invalid, wire::word};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Seek},
};

/// Maximum bytes before a token's terminating NUL (host resource policy).
pub const MAX_TOKEN_BYTES: usize = 4096;
pub const MAX_NODES: usize = 0x9000;
pub const MAX_DEFINITIONS: usize = 0x7fff;
/// Host limit, not a recovered engine reference-array capacity.
pub const MAX_REFERENCES: usize = 0x10000;
pub const MAX_STRING_BYTES: usize = 64 * 1024 * 1024;

/// Decoded cache tables. Free node slots and uninterpreted words are retained.
#[derive(Debug, Clone)]
pub struct Scenario {
    pub(crate) nodes: Vec<Node>,
    pub(crate) strings: Vec<u8>,
    pub(crate) scripts: Vec<Script>,
    pub(crate) globals: Vec<Global>,
    pub(crate) references: Vec<Reference>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Validation {
    pub slots: usize,
    pub live_nodes: usize,
    pub free_slots: usize,
    pub builtin_calls: usize,
    pub script_calls: usize,
    pub rooted_nodes: usize,
    pub unrooted_live_nodes: usize,
}

impl Scenario {
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }
    pub fn strings(&self) -> &[u8] {
        &self.strings
    }
    pub fn scripts(&self) -> &[Script] {
        &self.scripts
    }
    pub fn globals(&self) -> &[Global] {
        &self.globals
    }
    pub fn references(&self) -> &[Reference] {
        &self.references
    }
    pub fn script(&self, name: &str) -> Result<&Script> {
        let mut matches = self.scripts.iter().filter(|s| s.name().ok() == Some(name));
        let result = matches.next().ok_or_else(|| invalid("script not found"))?;
        if matches.next().is_some() {
            return Err(invalid("ambiguous script name"));
        }
        Ok(result)
    }

    /// Load only the root scenario's payload; validate reference IDs/classes
    /// through the map's existing salted tag index and group ancestry.
    pub fn read<R: Read + Seek>(map: &mut map::Map<R>) -> Result<Self> {
        let tag = map
            .scenario()
            .ok_or_else(|| invalid("map has no scenario"))?;
        let id = tag.id;
        let base = tag.location.ok_or(map::Error::NoTagData)?.address;
        let bytes = map.read_tag_bytes(id)?;
        let scenario = Self::decode(&bytes, base)?;
        for reference in &scenario.references {
            if reference.id == map::TagId::NULL {
                continue;
            }
            let tag = map.tag(reference.id)?;
            if tag.group != reference.group
                && !map.groups().iter().any(|g| {
                    g.class == tag.group
                        && (g.parent == Some(reference.group)
                            || g.grandparent == Some(reference.group))
                })
            {
                return Err(invalid(format!(
                    "script reference {} has incompatible tag class",
                    reference.id
                )));
            }
        }
        Ok(scenario)
    }

    /// Decode a complete scnr payload, with its guest virtual base. Every
    /// nonempty table must be within that payload, not just somewhere in a map.
    pub fn decode(bytes: &[u8], base: u32) -> Result<Self> {
        if bytes.len() < 0x240 {
            return Err(invalid("scenario header is truncated"));
        }
        if word(bytes, 0x1a8) != 0 {
            return Err(invalid(
                "legacy syntax-data blob is unsupported; retail uses +0x238",
            ));
        }
        let nodes = table(bytes, base, 0x238, Node::SIZE)?;
        // Preflight EVERY count and span before decoding or allocating any table.
        let strings = table(bytes, base, 0x1b0, 1)?;
        let scripts = table(bytes, base, 0x1b8, Script::SIZE)?;
        let globals = table(bytes, base, 0x1c0, Global::SIZE)?;
        let references = table(bytes, base, 0x1c8, Reference::SIZE)?;
        let scenario = Self {
            nodes: nodes
                .as_chunks::<{ Node::SIZE }>()
                .0
                .iter()
                .map(|b| Node::decode(b))
                .collect::<Result<_>>()?,
            strings: strings.to_vec(),
            scripts: scripts
                .as_chunks::<{ Script::SIZE }>()
                .0
                .iter()
                .map(|b| Script::decode(b))
                .collect::<Result<_>>()?,
            globals: globals
                .as_chunks::<{ Global::SIZE }>()
                .0
                .iter()
                .map(|b| Global::decode(b))
                .collect::<Result<_>>()?,
            references: references
                .as_chunks::<{ Reference::SIZE }>()
                .0
                .iter()
                .map(|b| Reference::decode(b))
                .collect::<Result<_>>()?,
        };
        scenario.validate_graph()?;
        Ok(scenario)
    }

    pub fn node(&self, datum: Datum) -> Result<&Node> {
        let node = self
            .nodes
            .get(datum.slot())
            .ok_or_else(|| invalid(format!("node {:#010x} is out of bounds", datum.0)))?;
        if datum == Datum::NULL || !node.live() || node.salt != datum.salt() {
            return Err(invalid(format!("stale/free node datum {:#010x}", datum.0)));
        }
        Ok(node)
    }
    pub fn string_bytes(&self, offset: u32) -> Result<&[u8]> {
        let rest = self
            .strings
            .get(offset as usize..)
            .ok_or_else(|| invalid(format!("string offset {offset:#x} is out of bounds")))?;
        let end = rest
            .iter()
            .take(MAX_TOKEN_BYTES + 1)
            .position(|&b| b == 0)
            .ok_or_else(|| invalid("unterminated script string or token exceeds 4096 bytes"))?;
        Ok(&rest[..end])
    }
    pub fn string(&self, offset: u32) -> Result<&str> {
        std::str::from_utf8(self.string_bytes(offset)?)
            .map_err(|_| invalid("non-UTF-8 script symbol"))
    }
    pub fn roots(&self) -> impl Iterator<Item = Datum> + '_ {
        self.globals
            .iter()
            .map(|g| g.root)
            .chain(self.scripts.iter().map(|s| s.root))
    }

    /// Full graph validation, including unreachable live slots. Iterative DFS
    /// rejects child/sibling cycles without risking the host call stack.
    pub fn validate_graph(&self) -> Result<()> {
        for root in self.roots() {
            if self.node(root)?.next != Datum::NULL {
                return Err(invalid("definition root has a sibling"));
            }
        }
        for (i, n) in self.nodes.iter().enumerate().filter(|(_, n)| n.live()) {
            if !matches!(n.flags & !8, 0 | 1 | 2 | 5) {
                return Err(invalid(format!(
                    "node {i}: unsupported flags {:#x}",
                    n.flags
                )));
            }
            if n.next != Datum::NULL {
                self.node(n.next)?;
            }
            if !n.atom() {
                let head = self.node(Datum(n.data))?;
                if !head.atom() || head.global() || head.value_type != 2 || head.opcode != n.opcode
                {
                    return Err(invalid(format!("node {i}: invalid function-name child")));
                }
            }
        }
        let mut color = vec![0u8; self.nodes.len()];
        for i in 0..self.nodes.len() {
            if !self.nodes[i].live() || color[i] == 2 {
                continue;
            }
            let mut stack = vec![(i, false)];
            while let Some((i, exit)) = stack.pop() {
                if exit {
                    color[i] = 2;
                    continue;
                }
                match color[i] {
                    1 => return Err(invalid(format!("syntax cycle at slot {i}"))),
                    2 => continue,
                    _ => color[i] = 1,
                }
                stack.push((i, true));
                let n = self.nodes[i];
                if n.next != Datum::NULL {
                    stack.push((n.next.slot(), false));
                }
                if !n.atom() {
                    stack.push((Datum(n.data).slot(), false));
                }
            }
        }
        Ok(())
    }

    /// All nodes reached structurally from roots; also follows script calls.
    /// This is conservative static reachability, not a runtime execution trace.
    pub fn reachable(&self, roots: impl IntoIterator<Item = Datum>) -> Result<BTreeSet<Datum>> {
        let mut seen = BTreeSet::new();
        let mut stack: Vec<_> = roots.into_iter().collect();
        while let Some(datum) = stack.pop() {
            if datum == Datum::NULL || !seen.insert(datum) {
                continue;
            }
            let n = self.node(datum)?;
            stack.push(n.next);
            if !n.atom() {
                stack.push(Datum(n.data));
            }
            if n.script_call() {
                let script = self
                    .scripts
                    .get(n.opcode as usize)
                    .ok_or_else(|| invalid("invalid script call index"))?;
                stack.push(script.root);
            }
        }
        Ok(seen)
    }

    pub fn builtin_names(&self) -> Result<BTreeMap<u16, String>> {
        let mut names = BTreeMap::new();
        for n in self
            .nodes
            .iter()
            .filter(|n| n.live() && !n.atom() && !n.script_call())
        {
            let head = self.node(Datum(n.data))?;
            let name = self.string(head.string_offset)?;
            if name.is_empty() {
                return Err(invalid("empty builtin name"));
            }
            if name == crate::h1_ai_slots::CONTROL_TOKEN
                && crate::h1_ai_slots::Operation::from_marker(head.data)
                    .is_some_and(|operation| operation.slot() == n.opcode)
            {
                // A converted operation names its dispatch protocol. The
                // unmarked retail call supplies the descriptor's name.
                continue;
            }
            if names
                .insert(n.opcode, name.to_owned())
                .is_some_and(|old| old != name)
            {
                return Err(invalid(format!(
                    "conflicting name for builtin {}",
                    n.opcode
                )));
            }
        }
        Ok(names)
    }

    pub fn reached_builtins(
        &self,
        roots: impl IntoIterator<Item = Datum>,
    ) -> Result<BTreeSet<u16>> {
        Ok(self
            .reachable(roots)?
            .iter()
            .filter_map(|d| {
                let n = &self.nodes[d.slot()];
                (!n.atom() && !n.script_call()).then_some(n.opcode)
            })
            .collect())
    }

    pub fn validate(&self, catalog: &Catalog) -> Result<Validation> {
        self.validate_graph()?;
        for s in &self.scripts {
            if s.kind as usize >= catalog.script_kinds.len() {
                return Err(invalid("invalid script kind"));
            }
            catalog.type_name(s.return_type)?;
        }
        for g in &self.globals {
            catalog.type_name(g.value_type)?;
        }
        let mut stats = Validation {
            slots: self.nodes.len(),
            ..Validation::default()
        };
        for (i, n) in self.nodes.iter().enumerate() {
            if !n.live() {
                stats.free_slots += 1;
                continue;
            }
            stats.live_nodes += 1;
            catalog.type_name(n.value_type)?;
            if !n.atom() {
                if n.script_call() {
                    let s = self
                        .scripts
                        .get(n.opcode as usize)
                        .ok_or_else(|| invalid(format!("node {i}: script index out of bounds")))?;
                    if self.string(self.node(Datum(n.data))?.string_offset)? != s.name()? {
                        return Err(invalid(format!(
                            "node {i}: script call name/index mismatch"
                        )));
                    }
                    stats.script_calls += 1;
                } else {
                    let function = catalog.function(n.opcode)?;
                    if function.flags & 2 == 0 {
                        let head = self.node(Datum(n.data))?;
                        let mut argument = head.next;
                        let parameters = if matches!(
                            n.opcode,
                            crate::h1_ai_slots::CONTROL | crate::h1_ai_slots::VEHICLE_COUNT
                        ) && self.string(head.string_offset)? == crate::h1_ai_slots::CONTROL_TOKEN
                        {
                            let operation = crate::h1_ai_slots::Operation::from_marker(head.data)
                                .filter(|op| op.slot() == n.opcode)
                                .ok_or_else(|| invalid(format!("node {i}: unknown Halo 1 AI operation")))?;
                            operation.parameters()
                        } else {
                            function.parameters()
                        };
                        for &expected in parameters {
                            let actual = self.node(argument)?;
                            if actual.value_type != expected {
                                return Err(invalid(format!(
                                    "node {i}: builtin argument type mismatch"
                                )));
                            }
                            argument = actual.next;
                        }
                        if argument != Datum::NULL {
                            return Err(invalid(format!("node {i}: builtin arity mismatch")));
                        }
                    }
                    stats.builtin_calls += 1;
                }
            } else if n.global() {
                let index = crate::expression::global_word(n.data)?;
                if index & 0x8000 != 0 {
                    if (index & 0x7fff) as usize >= catalog.external_global_types.len() {
                        return Err(invalid(format!(
                            "node {i}: external global index out of bounds"
                        )));
                    }
                } else {
                    let g = self
                        .globals
                        .get(index as usize)
                        .ok_or_else(|| invalid(format!("node {i}: global index out of bounds")))?;
                    if self.string(n.string_offset)? != g.name()? {
                        return Err(invalid("global name/index mismatch"));
                    }
                }
                self.string(n.string_offset)?;
            } else {
                if n.value_type != 2 {
                    catalog.type_name(n.opcode)?;
                }
                self.literal(n)?; // Same source-type dispatch as the typed view and renderer.
            }
        }
        self.builtin_names()?;
        stats.rooted_nodes = self.reachable(self.roots())?.len();
        stats.unrooted_live_nodes = stats.live_nodes - stats.rooted_nodes;
        Ok(stats)
    }

    /// Exact byte comparison for every retained table, including dead slots,
    /// reserved words and name padding. Does not rewrite/relocate a cache.
    pub fn verify_codec_symmetry(&self, bytes: &[u8], base: u32) -> Result<()> {
        if bytes.len() < 0x240 {
            return Err(invalid("scenario header is truncated"));
        }
        let checks = [
            (
                0x238,
                Node::SIZE,
                self.nodes
                    .iter()
                    .flat_map(|n| n.encode())
                    .collect::<Vec<_>>(),
            ),
            (
                0x1b8,
                Script::SIZE,
                self.scripts.iter().flat_map(|s| s.encode()).collect(),
            ),
            (
                0x1c0,
                Global::SIZE,
                self.globals.iter().flat_map(|g| g.encode()).collect(),
            ),
            (
                0x1c8,
                Reference::SIZE,
                self.references.iter().flat_map(|r| r.encode()).collect(),
            ),
        ];
        for (offset, stride, encoded) in checks {
            if table(bytes, base, offset, stride)? != encoded {
                return Err(invalid(format!("table +{offset:#x} failed codec symmetry")));
            }
        }
        if table(bytes, base, 0x1b0, 1)? != self.strings {
            return Err(invalid("string data failed codec symmetry"));
        }
        Ok(())
    }
}
fn table(bytes: &[u8], base: u32, field: usize, stride: usize) -> Result<&[u8]> {
    let count = word(bytes, field) as usize;
    let limit = match field {
        0x238 => MAX_NODES,
        0x1b0 => MAX_STRING_BYTES,
        0x1b8 | 0x1c0 => MAX_DEFINITIONS,
        0x1c8 => MAX_REFERENCES,
        _ => return Err(invalid("unknown script table")),
    };
    if count > limit {
        return Err(invalid(format!(
            "table +{field:#x} count {count} exceeds limit {limit}"
        )));
    }
    if count == 0 {
        return Ok(&[]);
    }
    let address = word(bytes, field + 4);
    if address & 3 != 0 {
        return Err(invalid(format!("table +{field:#x} is unaligned")));
    }
    let offset = address
        .checked_sub(base)
        .ok_or_else(|| invalid("table precedes scenario"))? as usize;
    let len = count
        .checked_mul(stride)
        .ok_or_else(|| invalid("table length overflow"))?;
    let end = offset
        .checked_add(len)
        .ok_or_else(|| invalid("table end overflow"))?;
    bytes
        .get(offset..end)
        .ok_or_else(|| invalid(format!("table +{field:#x} exceeds scenario payload")))
}
