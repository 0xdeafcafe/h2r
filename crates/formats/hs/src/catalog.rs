use crate::{
    Result, Scenario, invalid,
    wire::{short, word},
};
use std::collections::BTreeMap;

/// Addresses verified for the documented retail XBE, not an H2V opcode set.
pub const FUNCTION_TABLE: u32 = 0x0047_44e0;
pub const FUNCTION_COUNT: u32 = 0x0044_f6bc;
pub const TYPE_NAMES: u32 = 0x0046_ff00;
pub const SCRIPT_KIND_NAMES: u32 = 0x0046_fff8;
pub const EXTERNAL_GLOBAL_TABLE: u32 = 0x0047_3468;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Builtin {
    pub(crate) descriptor: u32,
    pub(crate) return_type: u16,
    /// 0, 1, 2 observed. Flag 2 includes custom argument evaluators.
    pub(crate) flags: u16,
    pub(crate) evaluate: u32,
    /// Optional usage text, NOT a name pointer.
    pub(crate) usage: Option<String>,
    pub(crate) parameters: Vec<u16>,
    /// Learned only from a scenario call's function-name token.
    pub(crate) name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Catalog {
    pub(crate) functions: Vec<Builtin>,
    pub(crate) types: Vec<String>,
    pub(crate) script_kinds: Vec<String>,
    pub(crate) external_global_types: Vec<u16>,
}
impl Catalog {
    pub fn functions(&self) -> &[Builtin] {
        &self.functions
    }
    pub fn types(&self) -> &[String] {
        &self.types
    }
    pub fn script_kinds(&self) -> &[String] {
        &self.script_kinds
    }
    pub fn external_global_types(&self) -> &[u16] {
        &self.external_global_types
    }

    /// Read the supported retail executable. No table/string dump is compiled in.
    /// Version/entry/count/dispatch anchors reject unrelated binaries; this is
    /// not a cryptographic identity check or a general-purpose table scanner.
    pub fn from_xbe(x: &xbe::Xbe<'_>) -> Result<Self> {
        if x.certificate.title_id != 0x4d530064
            || x.entry != 0x002d0aee
            || x.certificate.version != 3
            || word(x.bytes_at(FUNCTION_COUNT, 4)?, 0) != 911
            || x.bytes_at(0x0020994d, 7)? != [0x8b, 0x14, 0x8d, 0xe0, 0x44, 0x47, 0x00]
        {
            return Err(invalid("unsupported XBE: retail HaloScript anchors differ"));
        }
        read_tables(
            &|at, len| Ok(x.bytes_at(at, len)?.to_vec()),
            &|at| x.section_containing(at).is_some_and(|s| s.flags.executable()),
        )
    }

    /// Read the same tables from a running game's memory: `read` gives
    /// `len` bytes at a guest address, `executable` says whether an
    /// address is in the program's code. The data tables are the
    /// executable's own, so the result equals [`Self::from_xbe`]'s. The
    /// builtin count anchors it; the code anchor `from_xbe` also checks
    /// is not read, since a recompiled image need not keep the code bytes.
    pub fn from_memory(
        read: &dyn Fn(u32, usize) -> Result<Vec<u8>>,
        executable: &dyn Fn(u32) -> bool,
    ) -> Result<Self> {
        if word(&read(FUNCTION_COUNT, 4)?, 0) != 911 {
            return Err(invalid("unsupported image: retail HaloScript anchors differ"));
        }
        read_tables(read, executable)
    }

    pub fn type_name(&self, ty: u16) -> Result<&str> {
        self.types
            .get(ty as usize)
            .map(String::as_str)
            .ok_or_else(|| invalid(format!("unknown type {ty}")))
    }
    pub fn function(&self, index: u16) -> Result<&Builtin> {
        self.functions
            .get(index as usize)
            .ok_or_else(|| invalid(format!("unknown builtin index {index}")))
    }

    /// Validate every live node, then merge names without partially changing the
    /// catalog on a conflicting spelling. Script-call heads are NOT builtins.
    pub fn observe(&mut self, scenario: &Scenario) -> Result<()> {
        scenario.validate(self)?;
        let names = scenario.builtin_names()?;
        for (&i, name) in &names {
            if self
                .function(i)?
                .name
                .as_ref()
                .is_some_and(|old| old != name)
            {
                return Err(invalid(format!("conflicting builtin name at index {i}")));
            }
        }
        for (i, name) in names {
            self.functions[i as usize].name = Some(name);
        }
        Ok(())
    }
    pub fn named_functions(&self) -> BTreeMap<u16, &str> {
        self.functions
            .iter()
            .enumerate()
            .filter_map(|(i, f)| f.name.as_deref().map(|n| (i as u16, n)))
            .collect()
    }
}
fn cstring(
    bytes_at: &dyn Fn(u32, usize) -> Result<Vec<u8>>,
    address: u32,
) -> Result<String> {
    let mut out = Vec::new();
    for i in 0..4096 {
        let at = address
            .checked_add(i)
            .ok_or_else(|| invalid("string address overflow"))?;
        let byte = bytes_at(at, 1)?[0];
        if byte == 0 {
            return String::from_utf8(out).map_err(|_| invalid("non-UTF-8 string"));
        }
        out.push(byte);
    }
    Err(invalid("unterminated string"))
}

impl Builtin {
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
    pub fn usage(&self) -> Option<&str> {
        self.usage.as_deref()
    }
    pub fn parameters(&self) -> &[u16] {
        &self.parameters
    }

    pub fn descriptor(&self) -> u32 {
        self.descriptor
    }
    pub fn return_type(&self) -> u16 {
        self.return_type
    }
    pub fn flags(&self) -> u16 {
        self.flags
    }
    pub fn evaluate(&self) -> u32 {
        self.evaluate
    }
}

fn read_tables(
    bytes_at: &dyn Fn(u32, usize) -> Result<Vec<u8>>,
    executable: &dyn Fn(u32) -> bool,
) -> Result<Catalog> {
    let read_names = |base: u32, count: u32| -> Result<Vec<String>> {
        (0..count)
            .map(|i| cstring(bytes_at, word(&bytes_at(base + i * 4, 4)?, 0)))
            .collect()
    };
    let types = read_names(TYPE_NAMES, 62)?;
    let script_kinds = read_names(SCRIPT_KIND_NAMES, 6)?;
    let mut functions = Vec::with_capacity(911);
    for i in 0..911 {
        let address = word(&bytes_at(FUNCTION_TABLE + i * 4, 4)?, 0);
        let h = bytes_at(address, 14)?;
        let return_type = short(&h, 0);
        let count = short(&h, 12) as usize;
        if return_type >= 62 || count > 32 {
            return Err(invalid(format!("builtin {i}: invalid type/parameter count")));
        }
        let parameters: Vec<_> = bytes_at(address + 14, count * 2)?
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| short(b, 0))
            .collect();
        if parameters.iter().any(|&p| p >= 62) {
            return Err(invalid(format!("builtin {i}: invalid parameter type")));
        }
        let evaluate = word(&h, 4);
        if evaluate != 0 && !executable(evaluate) {
            return Err(invalid(format!(
                "builtin {i}: evaluation pointer is not executable"
            )));
        }
        functions.push(Builtin {
            descriptor: address,
            return_type,
            flags: short(&h, 2),
            evaluate,
            usage: (word(&h, 8) != 0)
                .then(|| cstring(bytes_at, word(&h, 8)))
                .transpose()?,
            parameters,
            name: None,
        });
    }
    let mut external_global_types = Vec::with_capacity(0x41d);
    for i in 0..0x41d {
        let address = word(&bytes_at(EXTERNAL_GLOBAL_TABLE + i * 4, 4)?, 0);
        let t = short(&bytes_at(address, 2)?, 0);
        if t >= 62 {
            return Err(invalid("invalid external-global type"));
        }
        external_global_types.push(t);
    }
    Ok(Catalog {
        functions,
        types,
        script_kinds,
        external_global_types,
    })
}
