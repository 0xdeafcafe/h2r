//! The store's side of the `.h2variant` format 2 bundle
//! (`variants::portable`, which owns the format): a Live variant as a
//! bundle once its script's IR is at hand, and the bundle's script hash.

use content::Sha256;
use live_hle::game_variants::LiveVariant;
pub use variants::portable::{
    Author, Bundle, EXTENSION, FORMAT, MAX_IR, ScriptPart, VERSION, base64_decode, base64_encode,
};

/// The full hash a bundle's script carries.
pub fn script_hash(b: &Bundle) -> Option<Sha256> {
    b.script.as_ref().and_then(|s| s.hash.parse().ok())
}

/// The full hash a Live variant names for its script.
pub fn live_hash(v: &LiveVariant) -> Option<Sha256> {
    v.script_hash.as_deref().and_then(|h| h.parse().ok())
}

/// A Live variant as a format 2 bundle, with `ir` its script's IR (which
/// must hash to the variant's `script_hash`; None for a variant with no
/// script). Checked as every format 2 file is (`Bundle::integrity`).
pub fn from_live(v: &LiveVariant, ir: Option<&[u8]>) -> Result<Bundle, String> {
    let script = match (&v.script_hash, ir) {
        (None, _) => None,
        (Some(_), None) => {
            return Err("its script's IR is not on this machine yet".into());
        }
        (Some(h), Some(ir)) => Some(ScriptPart {
            name: v.script_name.clone().unwrap_or_default(),
            hash: h.clone(),
            ir_b64: base64_encode(ir),
            source: None,
            modules: Default::default(),
        }),
    };
    let record = unhex(v.record_hex.trim())
        .and_then(|r| variants::GameVariant::from_bytes(&r).ok())
        .ok_or("its record is not 0x130 bytes of hex")?;
    let mut b = Bundle {
        format: FORMAT.into(),
        version: VERSION,
        name: v.name.clone(),
        description: v.description.clone(),
        author: Some(Author {
            xuid: v.author.xuid.clone(),
            gamertag: v.author.gamertag.clone(),
        }),
        game_type: record.game_type().label(),
        engine: v.engine,
        record_hex: hex(record.bytes()),
        script,
    };
    if let Some((_, ir)) = b.integrity().map_err(|e| e.to_string())?.script {
        b.game_type = ir.header.game_type.row().label.into();
    }
    Ok(b)
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 || !s.is_ascii() {
        return None;
    }
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok())
        .collect()
}
