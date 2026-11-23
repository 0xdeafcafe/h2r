//! The scenario context a working copy is checked against: the value
//! types and script kinds, every builtin with its signature and where its
//! name comes from, the game's globals the map's scripts name, and the
//! scenario's names by type. Studio writes it as [`CONTEXT_FILE`] beside
//! the working copy (`app::hs_external`) and `h2 hs lsp` reads it there.
//! It is built from the user's own map and executable at run time, so it
//! lives in the data root and is never committed.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The context's file name, in the working copy's folder.
pub const CONTEXT_FILE: &str = "h2hs.json";

/// The context's format. A reader refuses another.
pub const CONTEXT_VERSION: u32 = 1;

/// One builtin of the executable.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuiltinInfo {
    pub index: u32,
    /// `None`: nothing names it; it is called as `#builtin:<index>`.
    #[serde(default)]
    pub name: Option<String>,
    pub returns: String,
    #[serde(default)]
    pub parameters: Vec<String>,
    /// It reads its own arguments: `parameters` is not its argument list.
    #[serde(default)]
    pub own_arguments: bool,
    /// The executable's usage text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<String>,
    /// The documentation listing's description, when one names it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    /// Where the name comes from: `map` or `doc`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
}

impl BuiltinInfo {
    /// The name, or `#builtin:<index>`.
    pub fn label(&self) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| format!("#builtin:{}", self.index))
    }

    /// `(name <type> <type>)`.
    pub fn call_form(&self) -> String {
        let mut out = format!("({}", self.label());
        for p in &self.parameters {
            out.push_str(&format!(" <{p}>"));
        }
        out.push(')');
        out
    }
}

/// A name and its value type.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedType {
    pub name: String,
    #[serde(rename = "type")]
    pub type_name: String,
}

/// Everything a working copy is checked against. Empty, only the syntax
/// is checked.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Context {
    pub version: u32,
    /// The map the working copy is of.
    #[serde(default)]
    pub map: String,
    #[serde(default)]
    pub types: Vec<String>,
    #[serde(default)]
    pub script_kinds: Vec<String>,
    /// The script kinds whose header names a return type
    /// (`(script static boolean name ...)`).
    #[serde(default)]
    pub typed_kinds: Vec<String>,
    #[serde(default)]
    pub builtins: Vec<BuiltinInfo>,
    /// The game's own globals the map's scripts name.
    #[serde(default)]
    pub external_globals: Vec<NamedType>,
    /// The value types that hold an object name (`object_name` and its
    /// kinds): each takes any name in `names["object_name"]`.
    #[serde(default)]
    pub object_types: Vec<String>,
    /// The scenario's names by value type: every object name, every
    /// trigger volume, and the other names the map's scripts spell.
    #[serde(default)]
    pub names: BTreeMap<String, Vec<String>>,
}

impl Context {
    /// Reads a context file's text; another format version is refused.
    pub fn parse(json: &str) -> Result<Context, String> {
        let ctx: Context = serde_json::from_str(json).map_err(|e| format!("{CONTEXT_FILE}: {e}"))?;
        if ctx.version != CONTEXT_VERSION {
            return Err(format!(
                "{CONTEXT_FILE}: format {} is not {CONTEXT_VERSION}: open the script from Studio again",
                ctx.version
            ));
        }
        Ok(ctx)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// No builtins and no types: nothing to check names against.
    pub fn is_empty(&self) -> bool {
        self.builtins.is_empty() && self.types.is_empty()
    }

    pub fn is_object_type(&self, ty: &str) -> bool {
        self.object_types.iter().any(|t| t == ty)
    }

    /// The scenario names a value of type `ty` can be, when the context
    /// lists them: every object name for an object type, else the names of
    /// that type.
    pub fn names_for(&self, ty: &str) -> Option<&[String]> {
        let key = if self.is_object_type(ty) { "object_name" } else { ty };
        self.names.get(key).map(Vec::as_slice)
    }
}
