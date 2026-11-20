//! Reading a call's params, with errors that say which param and why.

use agent_proto::RpcError;
use scenario_edit::BlockKind;
use serde_json::Value;
use studio_proto::code;

use super::err;

/// A call's params (always an object by the time a method sees them).
pub struct P<'a>(pub &'a Value);

fn bad(k: &str, want: &str, v: &Value) -> RpcError {
    err(
        code::INVALID_PARAMS,
        format!("{k}: expected {want}, not {v}"),
    )
}

impl P<'_> {
    pub fn get(&self, k: &str) -> Option<&Value> {
        self.0.get(k).filter(|v| !v.is_null())
    }
    pub fn has(&self, k: &str) -> bool {
        self.get(k).is_some()
    }
    fn need(&self, k: &str) -> Result<&Value, RpcError> {
        self.get(k)
            .ok_or_else(|| err(code::INVALID_PARAMS, format!("{k} is required")))
    }
    pub fn str(&self, k: &str) -> Result<&str, RpcError> {
        let v = self.need(k)?;
        v.as_str().ok_or_else(|| bad(k, "a string", v))
    }
    pub fn opt_str(&self, k: &str) -> Result<Option<&str>, RpcError> {
        self.get(k)
            .map(|v| v.as_str().ok_or_else(|| bad(k, "a string", v)))
            .transpose()
    }
    pub fn u64(&self, k: &str) -> Result<u64, RpcError> {
        let v = self.need(k)?;
        v.as_u64()
            .ok_or_else(|| bad(k, "a whole number of 0 or more", v))
    }
    pub fn opt_u64(&self, k: &str) -> Result<Option<u64>, RpcError> {
        self.get(k)
            .map(|v| {
                v.as_u64()
                    .ok_or_else(|| bad(k, "a whole number of 0 or more", v))
            })
            .transpose()
    }
    pub fn usize(&self, k: &str) -> Result<usize, RpcError> {
        self.u64(k).map(|n| n as usize)
    }
    pub fn opt_usize(&self, k: &str) -> Result<Option<usize>, RpcError> {
        self.opt_u64(k).map(|n| n.map(|n| n as usize))
    }
    pub fn i64(&self, k: &str) -> Result<i64, RpcError> {
        let v = self.need(k)?;
        v.as_i64().ok_or_else(|| bad(k, "a whole number", v))
    }
    pub fn opt_f32(&self, k: &str) -> Result<Option<f32>, RpcError> {
        self.get(k)
            .map(|v| {
                v.as_f64()
                    .filter(|f| f.is_finite())
                    .map(|f| f as f32)
                    .ok_or_else(|| bad(k, "a number", v))
            })
            .transpose()
    }
    pub fn bool(&self, k: &str) -> Result<bool, RpcError> {
        match self.get(k) {
            None => Ok(false),
            Some(v) => v.as_bool().ok_or_else(|| bad(k, "true or false", v)),
        }
    }
    pub fn vec3(&self, k: &str) -> Result<[f32; 3], RpcError> {
        vec3(k, self.need(k)?)
    }
    pub fn opt_vec3(&self, k: &str) -> Result<Option<[f32; 3]>, RpcError> {
        self.get(k).map(|v| vec3(k, v)).transpose()
    }
    /// The `block` param as a placement block.
    pub fn block(&self) -> Result<BlockKind, RpcError> {
        block_kind(self.str("block")?)
    }
    /// The `block` and `index` params: an entry that exists.
    pub fn entry(&self, app: &crate::App) -> Result<(BlockKind, usize), RpcError> {
        let kind = self.block()?;
        let index = self.usize("index")?;
        let s = super::scenario::open(app)?;
        let n = s.block(kind).len();
        if index >= n {
            return Err(err(
                code::INVALID_PARAMS,
                format!("{} has {n} entries; no index {index}", block_name(kind)),
            ));
        }
        Ok((kind, index))
    }
}

/// `[x, y, z]` of finite numbers.
pub fn vec3(k: &str, v: &Value) -> Result<[f32; 3], RpcError> {
    let a = v.as_array().filter(|a| a.len() == 3);
    let n: Option<Vec<f32>> = a.map(|a| {
        a.iter()
            .filter_map(|x| x.as_f64().filter(|f| f.is_finite()).map(|f| f as f32))
            .collect()
    });
    match n {
        Some(n) if n.len() == 3 => Ok([n[0], n[1], n[2]]),
        _ => Err(bad(k, "[x, y, z] of finite numbers", v)),
    }
}

/// A block by name, without case, spaces or underscores (`netgame_flags`,
/// `NetgameFlags`, `vehicles`).
pub fn block_kind(name: &str) -> Result<BlockKind, RpcError> {
    let want = squash(name);
    scenario_edit::layout::BLOCKS
        .iter()
        .map(|b| b.kind)
        .find(|k| squash(&format!("{k:?}")) == want)
        .ok_or_else(|| {
            let names: Vec<String> = scenario_edit::layout::BLOCKS
                .iter()
                .map(|b| block_name(b.kind))
                .collect();
            err(
                code::INVALID_PARAMS,
                format!("no block {name:?} (blocks: {})", names.join(", ")),
            )
        })
}

fn squash(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase()
}

/// A block's API name: `netgame_flags`.
pub fn block_name(kind: BlockKind) -> String {
    let mut out = String::new();
    for (i, c) in format!("{kind:?}").chars().enumerate() {
        if c.is_ascii_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}
