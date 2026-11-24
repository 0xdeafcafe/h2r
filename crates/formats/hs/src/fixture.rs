//! Invented script tables and an invented executable, for this crate's
//! tests and its users' (feature `fixture`). Every name, type, table and
//! address value here is made up or is the reader's own anchor; nothing is
//! read from a game file.

use crate::{Catalog, Datum, Global, Node, Reference, Scenario, Script};
use crate::{EXTERNAL_GLOBAL_TABLE, FUNCTION_COUNT, FUNCTION_TABLE, SCRIPT_KIND_NAMES, TYPE_NAMES};

/// The builtin count [`Catalog::from_xbe`] requires.
pub const BUILTINS: usize = 911;
/// The type count it reads.
pub const TYPES: usize = 62;

/// One invented builtin descriptor.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Builtin {
    pub returns: u16,
    pub flags: u16,
    pub parameters: Vec<u16>,
    pub usage: Option<String>,
}

/// An invented executable: types (padded to [`TYPES`] with
/// `invented_type_<i>`), script kinds, builtins (padded to [`BUILTINS`]
/// with the default) and the one type every external global has.
#[derive(Clone, Debug)]
pub struct Exe {
    pub types: Vec<String>,
    pub kinds: Vec<String>,
    pub builtins: Vec<Builtin>,
    pub external_type: u16,
}

impl Default for Exe {
    fn default() -> Self {
        Self {
            types: Vec::new(),
            kinds: [
                "startup",
                "dormant",
                "continuous",
                "static",
                "stub",
                "command_script",
            ]
            .map(str::to_owned)
            .to_vec(),
            builtins: Vec::new(),
            external_type: 0,
        }
    }
}

const SECTION: u32 = 0x0020_0000;
const RAW: u32 = 0x400;
const DESCRIPTORS: u32 = 0x0046_0000;
const DESCRIPTOR_STRIDE: u32 = 0x40;
const EXTERNAL: u32 = 0x0045_ff00;
const STRINGS: u32 = 0x0047_6000;
const END: u32 = 0x0048_0000;

impl Exe {
    /// The executable's bytes: one section, the reader's anchors, the
    /// tables at the addresses it reads.
    pub fn xbe(&self) -> Vec<u8> {
        fn put(b: &mut [u8], at: usize, value: u32) {
            b[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        let off = |a: u32| (a - SECTION + RAW) as usize;
        let mut b = vec![0; (END - SECTION + RAW) as usize];
        b[..4].copy_from_slice(b"XBEH");
        for (at, value) in [
            (0x104, 0x10000),
            (0x108, RAW),
            (0x10c, END),
            (0x118, 0x10200),
            (0x11c, 1),
            (0x120, 0x10300),
            (0x128, 0x002d0aee ^ 0xa8fc57ab),
            (0x208, 0x4d530064),
            (0x2ac, 3),
            (0x300, 4),
            (0x304, SECTION),
            (0x308, END - SECTION),
            (0x30c, RAW),
            (0x310, END - SECTION),
            (0x314, 0x10380),
        ] {
            put(&mut b, at, value);
        }
        b[off(0x20994d)..off(0x20994d) + 7]
            .copy_from_slice(&[0x8b, 0x14, 0x8d, 0xe0, 0x44, 0x47, 0]);
        put(&mut b, off(FUNCTION_COUNT), BUILTINS as u32);
        let mut strings = STRINGS;
        let mut string = |b: &mut Vec<u8>, s: &str| {
            let at = strings;
            assert!((at + s.len() as u32) < END, "invented strings overflow");
            b[off(at)..off(at) + s.len()].copy_from_slice(s.as_bytes());
            strings += s.len() as u32 + 1;
            at
        };
        for i in 0..TYPES {
            let name = self.type_name(i);
            let at = string(&mut b, &name);
            put(&mut b, off(TYPE_NAMES + i as u32 * 4), at);
        }
        assert_eq!(self.kinds.len(), 6, "six script kinds");
        for (i, k) in self.kinds.iter().enumerate() {
            let at = string(&mut b, k);
            put(&mut b, off(SCRIPT_KIND_NAMES + i as u32 * 4), at);
        }
        put(&mut b, off(EXTERNAL), u32::from(self.external_type));
        for i in 0..0x41d {
            put(&mut b, off(EXTERNAL_GLOBAL_TABLE + i * 4), EXTERNAL);
        }
        assert!(
            self.builtins.len() <= BUILTINS,
            "at most {BUILTINS} builtins"
        );
        let default = Builtin::default();
        for i in 0..BUILTINS {
            let f = self.builtins.get(i).unwrap_or(&default);
            assert!(
                f.parameters.len() <= 24,
                "builtin {i}: at most 24 parameters"
            );
            let at = DESCRIPTORS + i as u32 * DESCRIPTOR_STRIDE;
            put(&mut b, off(FUNCTION_TABLE + i as u32 * 4), at);
            put(
                &mut b,
                off(at),
                u32::from(f.returns) | (u32::from(f.flags) << 16),
            );
            put(&mut b, off(at + 4), SECTION + 0x10);
            let usage = f.usage.as_deref().map_or(0, |u| string(&mut b, u));
            put(&mut b, off(at + 8), usage);
            let o = off(at + 12);
            b[o..o + 2].copy_from_slice(&(f.parameters.len() as u16).to_le_bytes());
            for (k, p) in f.parameters.iter().enumerate() {
                b[o + 2 + k * 2..o + 4 + k * 2].copy_from_slice(&p.to_le_bytes());
            }
        }
        b
    }

    /// The catalog [`Catalog::from_xbe`] reads from [`Self::xbe`].
    pub fn catalog(&self) -> Catalog {
        let bytes = self.xbe();
        Catalog::from_xbe(&xbe::Xbe::parse(&bytes).expect("the invented XBE parses"))
            .expect("the invented XBE reads")
    }

    fn type_name(&self, i: usize) -> String {
        self.types
            .get(i)
            .cloned()
            .unwrap_or_else(|| format!("invented_type_{i}"))
    }
}

/// Invented script tables, built node by node.
#[derive(Clone, Debug)]
pub struct Tables {
    scenario: Scenario,
}

impl Default for Tables {
    fn default() -> Self {
        Self::new()
    }
}

fn fixed_name(name: &str) -> [u8; 32] {
    assert!(name.len() < 32, "{name}: names are under 32 bytes");
    let mut b = [0; 32];
    b[..name.len()].copy_from_slice(name.as_bytes());
    b
}

impl Tables {
    pub fn new() -> Self {
        Self {
            scenario: Scenario {
                nodes: Vec::new(),
                strings: Vec::new(),
                scripts: Vec::new(),
                globals: Vec::new(),
                references: Vec::new(),
            },
        }
    }

    /// The offset of `token` in the string data, added once.
    pub fn token(&mut self, token: &str) -> u32 {
        let strings = &mut self.scenario.strings;
        let mut at = 0;
        for s in strings.split(|&c| c == 0) {
            if s == token.as_bytes() && (at as usize) < strings.len() {
                return at;
            }
            at += s.len() as u32 + 1;
        }
        let at = strings.len() as u32;
        strings.extend_from_slice(token.as_bytes());
        strings.push(0);
        at
    }

    fn push(&mut self, mut node: Node) -> Datum {
        let slot = self.scenario.nodes.len();
        assert!(slot < 0x7fff, "invented tables stay small");
        let salt = 0x8000 | slot as u16;
        node.salt = salt;
        self.scenario.nodes.push(node);
        Datum((u32::from(salt) << 16) | slot as u32)
    }

    fn atom(ty: u16, flags: u16, string_offset: u32, data: u32) -> Node {
        Node {
            salt: 0,
            opcode: ty,
            value_type: ty,
            flags,
            next: Datum::NULL,
            string_offset,
            data,
        }
    }

    /// A symbol or string literal of `ty`, spelled `token`, with value bits.
    pub fn literal(&mut self, ty: u16, token: &str, data: u32) -> Datum {
        let at = self.token(token);
        self.push(Self::atom(ty, 9, at, data))
    }

    /// A literal with no token (void, boolean and numbers).
    pub fn value(&mut self, ty: u16, data: u32) -> Datum {
        self.push(Self::atom(ty, 9, u32::MAX, data))
    }

    /// A reference to scenario global `index` (`0x8000 | i` for external
    /// global `i`).
    pub fn global(&mut self, index: u16, name: &str, ty: u16) -> Datum {
        let at = self.token(name);
        self.push(Self::atom(ty, 13, at, u32::from(index)))
    }

    fn link(&mut self, head: Datum, args: &[Datum]) {
        let mut previous = head;
        for &a in args {
            assert_eq!(
                self.scenario.nodes[a.slot()].next,
                Datum::NULL,
                "an argument is used once"
            );
            self.scenario.nodes[previous.slot()].next = a;
            previous = a;
        }
    }

    fn calls(&mut self, opcode: u16, name: &str, ty: u16, flags: u16, args: &[Datum]) -> Datum {
        let at = self.token(name);
        let mut head = Self::atom(2, 9, at, 0);
        head.opcode = opcode;
        let head = self.push(head);
        self.link(head, args);
        self.push(Node {
            salt: 0,
            opcode,
            value_type: ty,
            flags,
            next: Datum::NULL,
            string_offset: u32::MAX,
            data: head.0,
        })
    }

    /// Give node `d` the type `ty` its parent wants, keeping its own
    /// (a literal's source type, a call's opcode): a cast.
    pub fn retype(&mut self, d: Datum, ty: u16) {
        self.scenario.nodes[d.slot()].value_type = ty;
    }

    /// A call of builtin `index`, spelled `name`, of type `ty`.
    pub fn call(&mut self, index: u16, name: &str, ty: u16, args: &[Datum]) -> Datum {
        self.calls(index, name, ty, 8, args)
    }

    /// A call of the scenario's script `index`.
    pub fn script_call(&mut self, index: u16, name: &str, ty: u16, args: &[Datum]) -> Datum {
        self.calls(index, name, ty, 10, args)
    }

    /// A free slot, poisoned as retail leaves them.
    pub fn free(&mut self) {
        let mut n = Node::decode(&[0xba; Node::SIZE]).expect("a node");
        n.salt = 0;
        self.scenario.nodes.push(n);
    }

    pub fn script(&mut self, name: &str, kind: u16, returns: u16, root: Datum) {
        self.scenario.scripts.push(Script {
            name_bytes: fixed_name(name),
            kind,
            return_type: returns,
            root,
        });
    }

    pub fn global_definition(&mut self, name: &str, ty: u16, root: Datum) {
        self.scenario.globals.push(Global {
            name_bytes: fixed_name(name),
            value_type: ty,
            reserved: 0,
            root,
        });
    }

    pub fn reference(&mut self, group: [u8; 4], id: u32) {
        self.scenario.references.push(Reference {
            group: map::FourCC(group),
            id: map::TagId(id),
        });
    }

    /// The tables as decoded.
    pub fn scenario(&self) -> Scenario {
        self.scenario.clone()
    }

    /// Write the tables into a scenario payload whose guest address is
    /// `base`, from payload offset `at` on, and point the scenario's five
    /// table descriptors at them.
    pub fn write(&self, payload: &mut [u8], base: u32, at: usize) {
        let s = &self.scenario;
        let mut cursor = at.next_multiple_of(4);
        let mut place = |payload: &mut [u8], field: usize, count: usize, bytes: Vec<u8>| {
            let end = cursor + bytes.len();
            assert!(end <= payload.len(), "the tables overrun the payload");
            payload[cursor..end].copy_from_slice(&bytes);
            let address = if count == 0 { 0 } else { base + cursor as u32 };
            payload[field..field + 4].copy_from_slice(&(count as u32).to_le_bytes());
            payload[field + 4..field + 8].copy_from_slice(&address.to_le_bytes());
            cursor = end.next_multiple_of(4);
        };
        place(payload, 0x1b0, s.strings.len(), s.strings.clone());
        place(
            payload,
            0x1b8,
            s.scripts.len(),
            s.scripts.iter().flat_map(|x| x.encode()).collect(),
        );
        place(
            payload,
            0x1c0,
            s.globals.len(),
            s.globals.iter().flat_map(|x| x.encode()).collect(),
        );
        place(
            payload,
            0x1c8,
            s.references.len(),
            s.references.iter().flat_map(|x| x.encode()).collect(),
        );
        place(
            payload,
            0x238,
            s.nodes.len(),
            s.nodes.iter().flat_map(|x| x.encode()).collect(),
        );
    }
}

/// Write recordings into a scenario payload whose guest address is
/// `base`: the elements at payload offset `at`, each stream after them,
/// and the scenario's block descriptor (`+0x110`) pointing at the elements.
pub fn write_recordings(
    payload: &mut [u8],
    base: u32,
    at: usize,
    rows: &[crate::recording::Recorded],
) {
    let at = at.next_multiple_of(4);
    let mut cursor = at + rows.len() * crate::recording::ELEMENT;
    for (i, r) in rows.iter().enumerate() {
        let row = at + i * crate::recording::ELEMENT;
        let e = &mut payload[row..row + crate::recording::ELEMENT];
        e.fill(0);
        e[..r.name.len()].copy_from_slice(r.name.as_bytes());
        e[0x20] = r.version;
        e[0x22] = r.control_version;
        e[0x24..0x26].copy_from_slice(&r.ticks.to_le_bytes());
        e[0x2c..0x30].copy_from_slice(&(r.stream.len() as u32).to_le_bytes());
        if !r.stream.is_empty() {
            let end = cursor + r.stream.len();
            assert!(end <= payload.len(), "the recordings overrun the payload");
            payload[cursor..end].copy_from_slice(&r.stream);
            let e = &mut payload[row..row + crate::recording::ELEMENT];
            e[0x30..0x34].copy_from_slice(&(base + cursor as u32).to_le_bytes());
            cursor = end.next_multiple_of(4);
        }
    }
    let address = if rows.is_empty() { 0 } else { base + at as u32 };
    payload[0x110..0x114].copy_from_slice(&(rows.len() as u32).to_le_bytes());
    payload[0x114..0x118].copy_from_slice(&address.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_invented_executable_reads_back_as_built() {
        let exe = Exe {
            types: vec!["void".into(), "short".into()],
            builtins: vec![
                Builtin::default(),
                Builtin {
                    returns: 1,
                    flags: 2,
                    parameters: vec![1, 0],
                    usage: Some("invented help".into()),
                },
            ],
            ..Exe::default()
        };
        let c = exe.catalog();
        assert_eq!(c.functions().len(), BUILTINS);
        assert_eq!(c.type_name(1).unwrap(), "short");
        assert_eq!(c.type_name(5).unwrap(), "invented_type_5");
        assert_eq!(c.function(1).unwrap().parameters(), [1, 0]);
        assert_eq!(c.function(1).unwrap().flags(), 2);
        assert_eq!(c.function(1).unwrap().usage(), Some("invented help"));
        assert_eq!(c.script_kinds()[5], "command_script");
    }

    #[test]
    fn written_tables_decode_to_the_same_scenario() {
        let mut t = Tables::new();
        let a = t.value(1, 7);
        let root = t.call(1, "invented_call", 0, &[a]);
        t.script("main", 0, 0, root);
        let g = t.value(1, 2);
        t.global_definition("counter", 1, g);
        t.free();
        let mut payload = vec![0u8; 0x800];
        t.write(&mut payload, 0x9000_0000, 0x300);
        let s = Scenario::decode(&payload, 0x9000_0000).unwrap();
        assert_eq!(s.nodes(), t.scenario().nodes());
        assert_eq!(s.scripts(), t.scenario().scripts());
        assert_eq!(s.strings(), t.scenario().strings());
    }
}
