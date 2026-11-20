//! Borrowed typed views over immutable, salted syntax records.
use crate::{Datum, Node, Result, Scenario, Script, invalid};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobalIndex {
    Scenario(u16),
    External(u16),
}

/// Source literal plus preserved wire bits where interpretation is incomplete.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Literal<'a> {
    Void,
    Boolean(bool),
    /// IEEE-754 bits preserve negative zero and NaN payloads.
    RealBits(u32),
    Short(i16),
    Long(i32),
    String(&'a [u8]),
    FunctionName(&'a str),
    Symbol {
        source_type: u16,
        bits: u32,
        spelling: &'a str,
    },
}

#[derive(Debug)]
pub enum Expression<'a> {
    Call {
        function: u16,
        name: &'a str,
        result_type: u16,
        children: Children<'a>,
    },
    ScriptCall {
        index: u16,
        script: &'a Script,
        result_type: u16,
        children: Children<'a>,
    },
    Atom {
        source_type: u16,
        result_type: u16,
        literal: Literal<'a>,
    },
    Global {
        index: GlobalIndex,
        name: &'a str,
        result_type: u16,
    },
}

/// Argument datums in sibling order (the function-name node is excluded).
/// Resolve each with `Scenario::expression`; sharing does not duplicate storage.
#[derive(Debug)]
pub struct Children<'a> {
    scenario: &'a Scenario,
    next: Datum,
    remaining: usize,
}
impl Iterator for Children<'_> {
    type Item = Result<Datum>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.next == Datum::NULL {
            return None;
        }
        let current = self.next;
        if self.remaining == 0 {
            self.next = Datum::NULL;
            return Some(Err(invalid("cyclic argument chain")));
        }
        self.remaining -= 1;
        match self.scenario.node(current) {
            Ok(n) => {
                self.next = n.next;
                Some(Ok(current))
            }
            Err(e) => {
                self.next = Datum::NULL;
                Some(Err(e))
            }
        }
    }
}
impl Scenario {
    /// Decode one expression; no recursive expansion. Call `validate(catalog)`
    /// before VM execution for descriptor and whole-graph checks.
    pub fn expression(&self, datum: Datum) -> Result<Expression<'_>> {
        let n = self.node(datum)?;
        if !n.atom() {
            let head = self.node(Datum(n.data))?;
            let children = Children {
                scenario: self,
                next: head.next,
                remaining: self.nodes.len(),
            };
            if n.script_call() {
                let script = self
                    .scripts
                    .get(n.opcode as usize)
                    .ok_or_else(|| invalid("invalid script index"))?;
                Ok(Expression::ScriptCall {
                    index: n.opcode,
                    script,
                    result_type: n.value_type,
                    children,
                })
            } else {
                Ok(Expression::Call {
                    function: n.opcode,
                    name: self.string(head.string_offset)?,
                    result_type: n.value_type,
                    children,
                })
            }
        } else if n.global() {
            let raw = global_word(n.data)?;
            let index = if raw & 0x8000 == 0 {
                GlobalIndex::Scenario(raw)
            } else {
                GlobalIndex::External(raw & 0x7fff)
            };
            Ok(Expression::Global {
                index,
                name: self.string(n.string_offset)?,
                result_type: n.value_type,
            })
        } else {
            Ok(Expression::Atom {
                source_type: n.opcode,
                result_type: n.value_type,
                literal: self.literal(n)?,
            })
        }
    }

    pub(crate) fn literal(&self, n: &Node) -> Result<Literal<'_>> {
        if n.value_type == 2 {
            return Ok(Literal::FunctionName(self.string(n.string_offset)?));
        }
        Ok(match n.opcode {
            4 => Literal::Void,
            5 => Literal::Boolean(n.data as u8 != 0),
            6 => Literal::RealBits(n.data),
            7 => Literal::Short(n.data as i16),
            8 => Literal::Long(n.data as i32),
            9 => Literal::String(self.string_bytes(n.string_offset)?),
            ty => Literal::Symbol {
                source_type: ty,
                bits: n.data,
                spelling: self.string(n.string_offset)?,
            },
        })
    }
}

/// Retail external globals can be zero- or sign-extended i16 values.
/// Accept only those encodings, never arbitrary truncation of the upper word.
pub(crate) fn global_word(bits: u32) -> Result<u16> {
    let low = bits as u16;
    if bits == u32::from(low) || bits == (low as i16 as i32 as u32) {
        Ok(low)
    } else {
        Err(invalid("noncanonical global index high bits"))
    }
}
