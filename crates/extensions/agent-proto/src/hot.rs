//! Hot iteration methods (docs/graphics/hot-iteration.md): live tag edits
//! (`edit.*`) and the host renderer's overrides, parameters and toggles
//! (`render.*`). Host methods: answered where the game's host registers
//! them (crates/extensions/agent's `hot`), so `session.methods` lists them there.
//!
//! Params and results are JSON objects, documented per method below and
//! in the design page; tag and program ids are hex strings.

/// Method names.
pub mod method {
    pub const EDIT_MAP: &str = "edit.map";
    /// The current bytes inside one loaded tag: `{at, len, epoch}` →
    /// `{at, holds_bytes}` (numeric address, bytes in hex).
    pub const EDIT_READ: &str = "edit.read";
    /// Field writes, raw writes, evictions and block moves at one boundary
    /// under one journal id: `{writes, raw?: [{owner, offset, bytes, was?}],
    /// evict?: [owner], moves?: [{root, stride, count, capacity?, bytes,
    /// was}], label?, epoch, confirm_peers?}` (docs/runtime/agent-api.md,
    /// "Raw edits" and "Block moves").
    pub const EDIT_BATCH: &str = "edit.batch";
    /// An extent past the end of a map file, for a grown raw block:
    /// `{file?, bytes, align?, epoch}` → `{id, pointer, len}`.
    pub const EDIT_RAW_APPEND: &str = "edit.raw.append";
    /// Bytes held on the host for a later `{stage, at, len}`: `{stage?,
    /// bytes}` → `{stage, len}`.
    pub const EDIT_STAGE_PUT: &str = "edit.stage.put";
    /// What the raw overlay holds: `{entries, bytes, appended,
    /// appended_bytes, owners}`.
    pub const EDIT_OVERLAY: &str = "edit.overlay";
    /// The edit arena: `{base, len, mapped, used, largest_free,
    /// physical_free, physical_floor, moves}` (docs/runtime/agent-api.md,
    /// "Block moves").
    pub const EDIT_ARENA: &str = "edit.arena";
    /// The loaded map's tags: `{"group"?: "shad", "name"?: "<substring>",
    /// "limit"?: 200}` → `{"scenario", "names", "total", "tags": [{"tag",
    /// "group", "name", "address", "size", "defined"}]}`.
    pub const EDIT_TAGS: &str = "edit.tags";
    /// A field by path, or an element's fields: `{"tag": "0xe1230045" |
    /// "<name>", "group"?, "path"?: "postprocess[0].colors[1].color", "epoch"?}`.
    pub const EDIT_GET: &str = "edit.get";
    /// Writes a field, journalled: `{"tag", "group"?, "path", "value"}` →
    /// the change (`{"id", "old", "new", ...}`).
    pub const EDIT_SET: &str = "edit.set";
    /// The journal: every change, `live` (still holds) and `stale` (made on
    /// another map).
    pub const EDIT_CHANGES: &str = "edit.changes";
    /// Puts back one change (`{"id": 3}`) or all (no params).
    pub const EDIT_REVERT: &str = "edit.revert";

    /// The override directory, what is loaded and how each fared.
    pub const RENDER_OVERRIDES: &str = "render.overrides";
    /// Reads the override directory again (`{"dir"?: "<path>"}` moves it).
    pub const RENDER_RELOAD: &str = "render.reload";
    /// The translated programs, by id: `{"vertex"?: "microcode",
    /// "limit"?: 500}`.
    pub const RENDER_PROGRAMS: &str = "render.programs";
    /// Writes translated WGSL, the start of an override: `{"program"?:
    /// "<id>", "to"?: "<dir>"}` (all programs to `<override dir>/translated`
    /// by default). Served at the next frame; `render.overrides` reports it.
    pub const RENDER_DUMP: &str = "render.dump";
    /// Parameters: none → the list; `{"name"}` → one; `{"name", "value":
    /// 0.5}` sets it; `{"name", "value": null}` resets it.
    pub const RENDER_PARAM: &str = "render.param";
    /// Toggles: none → the list; `{"name", "on": false}` sets it;
    /// `{"name", "on": null}` resets it.
    pub const RENDER_TOGGLE: &str = "render.toggle";

    /// Every hot method.
    pub const ALL: &[&str] = &[
        EDIT_MAP,
        EDIT_READ,
        EDIT_BATCH,
        EDIT_RAW_APPEND,
        EDIT_STAGE_PUT,
        EDIT_OVERLAY,
        EDIT_ARENA,
        EDIT_TAGS,
        EDIT_GET,
        EDIT_SET,
        EDIT_CHANGES,
        EDIT_REVERT,
        RENDER_OVERRIDES,
        RENDER_RELOAD,
        RENDER_PROGRAMS,
        RENDER_DUMP,
        RENDER_PARAM,
        RENDER_TOGGLE,
    ];
}

/// Where overrides are read from when nothing says: `$H2_RENDER_OVERRIDES`,
/// else `render-overrides` in the cache directory ([`crate::cache_dir`]).
pub fn default_override_dir() -> std::path::PathBuf {
    match std::env::var_os("H2_RENDER_OVERRIDES").filter(|v| !v.is_empty()) {
        Some(d) => d.into(),
        None => crate::cache_dir().join("render-overrides"),
    }
}

/// Decodes standard base64 (`A-Z a-z 0-9 + /`, padding optional), the
/// encoding of raw bytes in `edit.batch`, `edit.raw.append` and
/// `edit.stage.put`.
pub fn decode_base64(s: &str) -> Result<Vec<u8>, String> {
    let s = s.trim_end_matches('=');
    if s.len() % 4 == 1 {
        return Err("base64: a stray final character".into());
    }
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for (i, c) in s.bytes().enumerate() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return Err(format!("base64: {:?} at {i} is not in the alphabet", c as char)),
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

/// Encodes `bytes` as standard base64 with padding.
pub fn encode_base64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips_every_tail_length() {
        for len in 0..10u8 {
            let bytes: Vec<u8> = (0..len).map(|i| i.wrapping_mul(97)).collect();
            let text = encode_base64(&bytes);
            assert_eq!(decode_base64(&text).unwrap(), bytes);
            assert_eq!(decode_base64(text.trim_end_matches('=')).unwrap(), bytes);
        }
        assert_eq!(encode_base64(b"Man"), "TWFu");
        assert_eq!(decode_base64("TWE=").unwrap(), b"Ma");
        assert!(decode_base64("TW!u").is_err());
        assert!(decode_base64("TWFuT").is_err());
    }
}
