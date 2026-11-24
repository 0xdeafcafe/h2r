//! Bytecode to [`Tree`]: the interpreter state (frame shift, three frame
//! offsets, key offset) is carried through the program and folded into the
//! nodes, one node per reached `(pc, state)` pair, in DFS order.
//!
//! Rescales (`01..04`), jumps (`05..07`), key offsets (`09..0b`) and
//! property 0 writes (`60`, `64`, `68`) emit nothing: the box interpreter
//! `0x002ff8b0` keeps the property in its stack state and never reads it
//! for keys (`mopp::boxvm`). The tests then become compares of the
//! full-precision corners against thresholds (section 4.3's table):
//!
//! | Opcode | Node |
//! |---|---|
//! | `10..12`, `23..25` | [`Node::Split`]: low when `lo < (Lop + off) << s`, high when `hi >= (Hop + off) << s` |
//! | `20..22` | [`Node::Split`]: low when `lo < (S + off + 1) << s`, high when `hi >= (S + off) << s` |
//! | `13..1c` | [`Node::Diagonal`]: shifted corner sums against folded constants |
//! | `26..28` | [`Node::Bound`]: on when `hi >= (l + off - 1) << s` and `lo < (h + off) << s` |
//! | `29..2b` | [`Node::Bound`]: on when `hi >= min` and `lo < max + 1` |
//! | `30..53` | [`Node::Leaf`]: the key plus the key offset |
//!
//! Everything else the original would fault or assert on (an opcode
//! outside the table, a property slot past the state, an operand or
//! opcode past the program's size, a rescale below shift 0) becomes a
//! [`Node::Trap`]: a query that reaches it bails and the lifted code runs.
//! A program that unfolds past [`super::NODES_PER_BYTE`] nodes per byte is
//! `guest_only`: every query on it bails.
use super::{HEADER_BYTES, NODES_PER_BYTE, Source};
mod store;
use store::Nodes;
use ::mopp::boxvm::diagonal_plane;
use ::mopp::code::{ROOT_SHIFT, terminal_key};

/// No child: the path ends there.
pub const NONE: u32 = u32::MAX;

/// One decoded test. Thresholds are on the full-precision corners `lo`
/// and `hi` the setup computes; children are node indices, the low child
/// always the next index when present.
#[derive(Clone, Copy, Debug, PartialEq, Eq, bytemuck::CheckedBitPattern)]
#[repr(u8)]
pub enum Node {
    /// The low child when `lo[axis] < below`, the high child when
    /// `hi[axis] >= above`, the low child's subtree first.
    Split {
        axis: u8,
        below: i64,
        above: i64,
        low: u32,
        high: u32,
    },
    /// A diagonal split (`13..1c`). With the corners shifted right by
    /// `shift`, `lower` sums `lo` over the plane's positive axes minus `hi`
    /// over its negative ones and `upper` the reverse: the low child when
    /// `lower < below`, the high child when `upper > above`.
    Diagonal {
        op: u8,
        shift: u8,
        below: i64,
        above: i64,
        low: u32,
        high: u32,
    },
    /// The path goes on to `next` only when `hi[axis] >= above` and
    /// `lo[axis] < below`.
    Bound {
        axis: u8,
        above: i64,
        below: i64,
        next: u32,
    },
    /// A terminal: the key the original appends, key offset folded in.
    Leaf { key: u32 },
    /// The original faults or asserts here; the query bails.
    Trap { pc: u32 },
}

/// A decoded program.
#[derive(Clone, Debug)]
pub struct Tree {
    code: u32,
    source: Source,
    nodes: Nodes,
    root: u32,
    guest_only: bool,
}

/// The interpreter state the decoder carries: frame offsets, bits taken by
/// rescales (the frame shift is `16 - bits`) and the key offset.
#[derive(Clone, Copy, Debug)]
struct State {
    offset: [i64; 3],
    bits: u32,
    key: u32,
}

impl State {
    fn shift(&self) -> u32 {
        ROOT_SHIFT - self.bits
    }

    /// `x << s` for the frame value `x`, as a full-precision threshold.
    fn full(&self, x: i64) -> i64 {
        x * (1i64 << self.shift())
    }
}

/// What one decode step produced from a `(pc, state)` pair.
enum Step {
    /// The path ends without a node (`00`, or a split's empty side).
    End,
    /// A node, and the `(pc, state)` pairs of its low and high children.
    Node(Node, Option<(usize, State)>, Option<(usize, State)>),
}

/// Big-endian operand bytes, or `None` past the program.
fn be(code: &[u8], at: usize, n: usize) -> Option<i64> {
    let bytes = code.get(at..at.checked_add(n)?)?;
    Some(bytes.iter().fold(0i64, |v, &b| (v << 8) | i64::from(b)))
}

fn byte(code: &[u8], at: usize) -> Option<i64> {
    be(code, at, 1)
}

/// Follow non-emitting opcodes from `pc` to the next node.
fn step(code: &[u8], mut pc: usize, mut st: State) -> Step {
    let trap = |pc: usize| Step::Node(Node::Trap { pc: pc as u32 }, None, None);
    // Jumps only go forward, so this ends within the program's length.
    loop {
        let Some(op) = code.get(pc).copied() else {
            return trap(pc);
        };
        let follow = |next: usize, st: State| Some((next, st));
        match op {
            0x00 => return Step::End,
            0x01..=0x04 => {
                let k = u32::from(op);
                if st.bits + k > ROOT_SHIFT {
                    return trap(pc);
                }
                let mut offset = [0i64; 3];
                for (a, o) in offset.iter_mut().enumerate() {
                    let Some(b) = byte(code, pc + 1 + a) else {
                        return trap(pc);
                    };
                    *o = (b + st.offset[a]) << k;
                }
                st.offset = offset;
                st.bits += k;
                pc += 4;
            }
            0x05..=0x07 => {
                let n = usize::from(op - 4);
                let Some(skip) = be(code, pc + 1, n) else {
                    return trap(pc);
                };
                pc += 1 + n + skip as usize;
            }
            0x09..=0x0b => {
                let n = 1usize << (op - 9);
                let Some(v) = be(code, pc + 1, n) else {
                    return trap(pc);
                };
                st.key = if n == 4 {
                    v as u32
                } else {
                    st.key.wrapping_add(v as u32)
                };
                pc += 1 + n;
            }
            0x60 | 0x64 | 0x68 => {
                let n = 1usize << ((op - 0x60) / 4);
                if be(code, pc + 1, n).is_none() {
                    return trap(pc);
                }
                pc += 1 + n;
            }
            0x10..=0x12 | 0x23..=0x25 => {
                let (axis, wide) = if op <= 0x12 {
                    (op - 0x10, false)
                } else {
                    (op - 0x23, true)
                };
                let a = usize::from(axis);
                let (Some(lop), Some(hop)) = (byte(code, pc + 1), byte(code, pc + 2)) else {
                    return trap(pc);
                };
                let (first, second) = if wide {
                    let (Some(f), Some(s)) = (be(code, pc + 3, 2), be(code, pc + 5, 2)) else {
                        return trap(pc);
                    };
                    (pc + 7 + f as usize, pc + 7 + s as usize)
                } else {
                    let Some(skip) = byte(code, pc + 3) else {
                        return trap(pc);
                    };
                    (pc + 4, pc + 4 + skip as usize)
                };
                let node = Node::Split {
                    axis,
                    below: st.full(lop + st.offset[a]),
                    above: st.full(hop + st.offset[a]),
                    low: NONE,
                    high: NONE,
                };
                return Step::Node(node, follow(first, st), follow(second, st));
            }
            0x20..=0x22 => {
                let axis = op - 0x20;
                let a = usize::from(axis);
                let (Some(split), Some(skip)) = (byte(code, pc + 1), byte(code, pc + 2)) else {
                    return trap(pc);
                };
                let first = pc + 3;
                let node = Node::Split {
                    axis,
                    below: st.full(split + st.offset[a] + 1),
                    above: st.full(split + st.offset[a]),
                    low: NONE,
                    high: NONE,
                };
                return Step::Node(node, follow(first, st), follow(first + skip as usize, st));
            }
            0x13..=0x1c => {
                let (Some(lop), Some(hop), Some(skip)) =
                    (byte(code, pc + 1), byte(code, pc + 2), byte(code, pc + 3))
                else {
                    return trap(pc);
                };
                let (signs, m, k) = diagonal_plane(op);
                // Frame values: L = (lo >> s) - off, H = (hi >> s) + 1 - off.
                // lower = sum+ L - sum- H, upper = sum+ H - sum- L; the
                // constants below are what the offsets and the +1 add.
                let (mut lower_c, mut upper_c) = (0i64, 0i64);
                for (a, &sign) in signs.iter().enumerate() {
                    let off = st.offset[a];
                    match sign {
                        1 => {
                            lower_c -= off;
                            upper_c += 1 - off;
                        }
                        -1 => {
                            lower_c += off - 1;
                            upper_c += off;
                        }
                        _ => {}
                    }
                }
                let first = pc + 4;
                let node = Node::Diagonal {
                    op,
                    shift: st.shift() as u8,
                    below: lop * m - k - lower_c,
                    above: hop * m - k - upper_c,
                    low: NONE,
                    high: NONE,
                };
                return Step::Node(node, follow(first, st), follow(first + skip as usize, st));
            }
            0x26..=0x28 => {
                let axis = op - 0x26;
                let a = usize::from(axis);
                let (Some(l), Some(h)) = (byte(code, pc + 1), byte(code, pc + 2)) else {
                    return trap(pc);
                };
                let node = Node::Bound {
                    axis,
                    above: st.full(l + st.offset[a] - 1),
                    below: st.full(h + st.offset[a]),
                    next: NONE,
                };
                return Step::Node(node, follow(pc + 3, st), None);
            }
            0x29..=0x2b => {
                let (Some(min), Some(max)) = (be(code, pc + 1, 3), be(code, pc + 4, 3)) else {
                    return trap(pc);
                };
                let node = Node::Bound {
                    axis: op - 0x29,
                    above: min,
                    below: max + 1,
                    next: NONE,
                };
                return Step::Node(node, follow(pc + 7, st), None);
            }
            0x30..=0x53 => {
                let Ok(key) = terminal_key(op, |n| be(code, pc + 1, n).map(|v| v as u32).ok_or(()))
                else {
                    return trap(pc);
                };
                let node = Node::Leaf {
                    key: st.key.wrapping_add(key),
                };
                return Step::Node(node, None, None);
            }
            _ => return trap(pc),
        }
    }
}

/// Which link of an emitted node a pending child fills.
#[derive(Clone, Copy)]
enum Link {
    Root,
    Low(u32),
    High(u32),
}

impl Tree {
    /// Decode the program at guest address `code` from its bytes.
    pub fn decode(code: u32, source: Source) -> Self {
        let limit = NODES_PER_BYTE * source.bytecode.len().max(1);
        let mut nodes: Vec<Node> = Vec::new();
        let mut root = NONE;
        let root_state = State {
            offset: [0; 3],
            bits: 0,
            key: 0,
        };
        // Pre-order with the high child pushed first, so each low child is
        // emitted right after its parent.
        let mut pending = vec![(0usize, root_state, Link::Root)];
        let mut guest_only = false;
        while let Some((pc, st, link)) = pending.pop() {
            let Step::Node(node, low, high) = step(&source.bytecode, pc, st) else {
                continue;
            };
            if nodes.len() >= limit {
                guest_only = true;
                break;
            }
            let index = nodes.len() as u32;
            match link {
                Link::Root => root = index,
                Link::Low(parent) => set_child(&mut nodes[parent as usize], false, index),
                Link::High(parent) => set_child(&mut nodes[parent as usize], true, index),
            }
            // Child links start as NONE and are filled when a child emits.
            nodes.push(node);
            if let Some((pc, st)) = high {
                pending.push((pc, st, Link::High(index)));
            }
            if let Some((pc, st)) = low {
                pending.push((pc, st, Link::Low(index)));
            }
        }
        if guest_only {
            nodes = Vec::new();
            root = NONE;
        }
        nodes.shrink_to_fit();
        Self {
            code,
            source,
            nodes: Nodes::Heap(nodes),
            root,
            guest_only,
        }
    }

    /// The code object pointer the tree was decoded from.
    pub fn code(&self) -> u32 {
        self.code
    }

    pub fn source(&self) -> &Source {
        &self.source
    }

    pub fn nodes(&self) -> &[Node] {
        self.nodes.as_slice()
    }

    /// The first node, or [`NONE`] for a program that ends at once.
    pub fn root(&self) -> u32 {
        self.root
    }

    /// Whether the program unfolded past the decode bound: every query on
    /// it bails.
    pub fn guest_only(&self) -> bool {
        self.guest_only
    }

    /// The guest bytes the original reads of this program: header and
    /// bytecode. An output overlapping them is an alias.
    pub fn span(&self) -> (u32, u32) {
        (self.code, HEADER_BYTES + self.source.bytecode.len() as u32)
    }

    /// Bytes held: nodes and the bytecode kept to verify the source.
    pub fn footprint(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.nodes.bytes()
            + self.source.bytecode.capacity()
    }
}

fn set_child(node: &mut Node, high_side: bool, index: u32) {
    match node {
        Node::Split { low, high, .. } | Node::Diagonal { low, high, .. } => {
            if high_side {
                *high = index;
            } else {
                *low = index;
            }
        }
        Node::Bound { next, .. } => *next = index,
        Node::Leaf { .. } | Node::Trap { .. } => unreachable!("a terminal has no children"),
    }
}
