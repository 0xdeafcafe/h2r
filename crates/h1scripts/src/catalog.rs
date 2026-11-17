//! Halo 1's builtin function, value type and script type tables, read from
//! Halo 1's own XBE (Xbox retail, title `4D530004`).
//!
//! Unlike Halo 2's descriptors, Halo 1's carry the function's name, so every
//! builtin is named straight from the executable. Evidence for each address
//! is in `docs/formats/h1-scripts.md`.

use crate::{Result, invalid, u16_at, u32_at};

/// The function definition pointer table. The dispatcher at `0x000f1d40`
/// indexes it (`mov eax, [eax*4 + 0x22dd80]`) and reads the parameter types
/// at definition `+0x1a`.
pub const FUNCTION_TABLE: u32 = 0x0022_dd80;
/// Where the function count is checked: `cmp si, 0x1a2` closes the loop over
/// [`FUNCTION_TABLE`] that starts at `0x000ec21f`.
pub const FUNCTION_COUNT_CHECK: u32 = 0x000e_c271;
/// The value type name pointer table.
pub const TYPE_NAMES: u32 = 0x0021_c4e0;
/// Value types: the names table holds 49 pointers, then the integer 5.
pub const TYPE_COUNT: usize = 49;
/// The script type name pointer table: startup, dormant, continuous, static,
/// stub. It sits just before the value type names.
pub const SCRIPT_KIND_NAMES: u32 = 0x0021_c260;
/// Script types.
pub const SCRIPT_KIND_COUNT: usize = 5;
/// Most parameters any definition declares.
const MAX_PARAMETERS: u16 = 32;

/// Value type indices used by name in this crate. The full list is read
/// from the XBE ([`Catalog::types`]).
pub mod types {
    /// `function name`: a call's head atom.
    pub const FUNCTION_NAME: u16 = 2;
    /// `passthrough`.
    pub const PASSTHROUGH: u16 = 3;
    /// `void`.
    pub const VOID: u16 = 4;
    /// `boolean`.
    pub const BOOLEAN: u16 = 5;
    /// `real`.
    pub const REAL: u16 = 6;
    /// `short`.
    pub const SHORT: u16 = 7;
    /// `long`.
    pub const LONG: u16 = 8;
    /// `string`.
    pub const STRING: u16 = 9;
    /// `script`.
    pub const SCRIPT: u16 = 10;
}

/// One builtin: `hs_function_definition`, `0x1a` bytes plus parameters.
///
/// | Offset | Field |
/// |---|---|
/// | `+0x00` | u16 return type |
/// | `+0x04` | name pointer |
/// | `+0x08` | parse function |
/// | `+0x0c` | evaluate function |
/// | `+0x10` | documentation pointer |
/// | `+0x14` | parameter help pointer |
/// | `+0x18` | u16 parameter count |
/// | `+0x1a` | u16 parameter types |
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Function {
    /// The name the compiler matches.
    pub name: String,
    /// Return type index.
    pub return_type: u16,
    /// Parse function address: the ordinary one for fixed signatures, a
    /// special one for special forms (`begin`, `if`, `set`, `sleep`, ...).
    pub parse: u32,
    /// Evaluate function address.
    pub evaluate: u32,
    /// Declared parameter types.
    pub parameters: Vec<u16>,
}

/// Halo 1's script tables, by index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    functions: Vec<Function>,
    types: Vec<String>,
    script_kinds: Vec<String>,
    /// The parse function shared by fixed-signature builtins.
    ordinary_parse: u32,
}

impl Catalog {
    /// Reads the tables from Halo 1's XBE. Fails unless the title, the count
    /// check and every table entry agree with what this crate knows.
    pub fn from_xbe(x: &xbe::Xbe<'_>) -> Result<Self> {
        if x.certificate.title_id != h1map::disc::HALO1_TITLE_ID {
            return Err(invalid(format!(
                "title {:08x} is not Halo 1",
                x.certificate.title_id
            )));
        }
        let check = x.bytes_at(FUNCTION_COUNT_CHECK, 5)?;
        if check[..3] != [0x66, 0x81, 0xfe] {
            return Err(invalid("function count check not found: unknown XBE"));
        }
        let count = u16::from_le_bytes([check[3], check[4]]) as usize;
        let mut functions = Vec::with_capacity(count);
        for i in 0..count as u32 {
            let def = x.read_u32(FUNCTION_TABLE + 4 * i)?;
            let head = x.bytes_at(def, 0x1a)?;
            let n = u16_at(head, 0x18).unwrap_or(0);
            if n > MAX_PARAMETERS {
                return Err(invalid(format!("function {i}: {n} parameters")));
            }
            let params = x.bytes_at(def + 0x1a, 2 * n as usize)?;
            functions.push(Function {
                name: cstring(x, u32_at(head, 4).unwrap_or(0))?,
                return_type: u16_at(head, 0).unwrap_or(0),
                parse: u32_at(head, 8).unwrap_or(0),
                evaluate: u32_at(head, 0xc).unwrap_or(0),
                parameters: (0..n as usize)
                    .map(|k| u16_at(params, 2 * k).unwrap_or(0))
                    .collect(),
            });
        }
        let names = |at: u32, n: usize| -> Result<Vec<String>> {
            (0..n as u32)
                .map(|i| cstring(x, x.read_u32(at + 4 * i)?))
                .collect()
        };
        // The word after the 49th name is a small integer, not a pointer.
        if x.read_u32(TYPE_NAMES + 4 * TYPE_COUNT as u32)? >= 0x1_0000 {
            return Err(invalid("value type table is not 49 entries: unknown XBE"));
        }
        Self::new(
            functions,
            names(TYPE_NAMES, TYPE_COUNT)?,
            names(SCRIPT_KIND_NAMES, SCRIPT_KIND_COUNT)?,
        )
    }

    /// A catalog from its parts, checked: every type index in range, every
    /// name non-empty and unique.
    pub fn new(
        functions: Vec<Function>,
        types: Vec<String>,
        script_kinds: Vec<String>,
    ) -> Result<Self> {
        let mut seen = std::collections::HashSet::new();
        for (i, f) in functions.iter().enumerate() {
            if f.name.is_empty() || !seen.insert(f.name.as_str()) {
                return Err(invalid(format!("function {i}: empty or repeated name")));
            }
            let ty_ok = |t: u16| (t as usize) < types.len();
            if !ty_ok(f.return_type) || !f.parameters.iter().all(|&t| ty_ok(t)) {
                return Err(invalid(format!("function {i} ({}): bad type", f.name)));
            }
        }
        // The ordinary parse function is the one most definitions share.
        let mut counts = std::collections::HashMap::<u32, usize>::new();
        for f in &functions {
            *counts.entry(f.parse).or_default() += 1;
        }
        let ordinary_parse = counts
            .into_iter()
            .max_by_key(|&(p, n)| (n, p))
            .map_or(0, |(p, _)| p);
        Ok(Self {
            functions,
            types,
            script_kinds,
            ordinary_parse,
        })
    }

    /// Every builtin, by index.
    pub fn functions(&self) -> &[Function] {
        &self.functions
    }
    /// Value type names, by index.
    pub fn types(&self) -> &[String] {
        &self.types
    }
    /// Script type names, by index.
    pub fn script_kinds(&self) -> &[String] {
        &self.script_kinds
    }
    /// Builtin `index`.
    pub fn function(&self, index: u16) -> Result<&Function> {
        self.functions
            .get(index as usize)
            .ok_or_else(|| invalid(format!("unknown builtin {index}")))
    }
    /// The index of the builtin called `name`.
    pub fn index_of(&self, name: &str) -> Option<u16> {
        self.functions
            .iter()
            .position(|f| f.name == name)
            .map(|i| i as u16)
    }
    /// Value type `ty`'s name.
    pub fn type_name(&self, ty: u16) -> Result<&str> {
        self.types
            .get(ty as usize)
            .map(String::as_str)
            .ok_or_else(|| invalid(format!("unknown value type {ty}")))
    }
    /// Script type `kind`'s name.
    pub fn script_kind(&self, kind: u16) -> Result<&str> {
        self.script_kinds
            .get(kind as usize)
            .map(String::as_str)
            .ok_or_else(|| invalid(format!("unknown script type {kind}")))
    }
    /// Whether builtin `index` is parsed with the ordinary parser, so its
    /// arguments match its declared parameters one for one. Special forms
    /// (`begin`, `if`, `set`, `sleep`, ...) have their own parsers.
    pub fn fixed_signature(&self, index: u16) -> bool {
        self.function(index)
            .is_ok_and(|f| f.parse == self.ordinary_parse)
    }
}

fn cstring(x: &xbe::Xbe<'_>, address: u32) -> Result<String> {
    let mut out = Vec::new();
    for i in 0..256 {
        let byte = x.bytes_at(address + i, 1)?[0];
        if byte == 0 {
            return String::from_utf8(out).map_err(|_| invalid("non-UTF-8 XBE string"));
        }
        out.push(byte);
    }
    Err(invalid(format!("unterminated XBE string at {address:#x}")))
}
