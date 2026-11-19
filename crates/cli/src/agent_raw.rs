//! `h2 agent raw <id> [parts]` (`world.raw`) and `h2 agent raw-diff <a>
//! <b>`: an object's raw state at two moments, and what changed between
//! them. `h2 agent poke` writes bytes within one part
//! (docs/runtime/agent-api.md, "world.poke").

use agent_proto::raw::{self, Raw, method};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

pub const USAGE: &str = "  raw <id> [part[,part...]] [len]
                         world.raw: the object's body, physics component,
                         Havok entity, motion and shape as hex (parts:
                         object physics_component entity motion shape);
                         save one before and one after with > a.json
  raw-diff <a.json> <b.json>
                         per part, the 32-bit words that differ between two
                         world.raw answers (and as floats where they read as
                         ones); no game needed
  poke <id> <part> <offset> <hex>
                         world.poke: claim control and write 1 to 64 bytes
                         at a tick boundary; offset is decimal or 0x hex";

/// A `raw` or `poke` call, or `None` for other words.
pub fn parse(words: &[String]) -> Option<Result<(String, Value)>> {
    match words.first()?.as_str() {
        "raw" => Some(parse_raw(&words[1..])),
        "poke" => Some(parse_poke(&words[1..])),
        _ => None,
    }
}

fn parse_raw(w: &[String]) -> Result<(String, Value)> {
    if w.len() > 3 {
        bail!("raw: unexpected {}", w[3]);
    }
    let id = w.first().context("raw needs an object id (player.0)")?;
    let mut p = json!({ "id": id });
    for a in &w[1..] {
        if let Ok(n) = a.parse::<u32>() {
            p["len"] = json!(n);
        } else if let Some(h) = a.strip_prefix("0x")
            && let Ok(n) = u32::from_str_radix(h, 16)
        {
            p["len"] = json!(n);
        } else {
            let parts: Vec<&str> = a.split(',').filter(|s| !s.is_empty()).collect();
            p["parts"] = json!(parts);
        }
    }
    Ok((method::RAW.into(), p))
}

fn parse_poke(words: &[String]) -> Result<(String, Value)> {
    let [id, part, offset, hex] = words else {
        bail!("usage: h2 agent poke <id> <part> <offset> <hex>");
    };
    let offset = match offset.strip_prefix("0x") {
        Some(digits) => u32::from_str_radix(digits, 16),
        None => offset.parse::<u32>(),
    }
    .context("poke offset must be a nonnegative integer in decimal or 0x hex")?;
    Ok((
        method::POKE.into(),
        json!({"id": id, "part": part, "offset": offset, "hex": hex}),
    ))
}

/// A `world.raw` answer from a file: the result as `h2 agent` prints it,
/// or a whole JSON-RPC response; in a script's output, the last line.
fn load(path: &str) -> Result<Raw> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {path}"))?;
    parse_answer(&text).with_context(|| format!("{path}: not a world.raw answer"))
}

fn parse_answer(text: &str) -> Result<Raw> {
    let v: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        // One JSON value per line (a script): the last that parses.
        Err(_) => text
            .lines()
            .rev()
            .find_map(|l| serde_json::from_str(l).ok())
            .context("no JSON in it")?,
    };
    let v = match v.get("result") {
        Some(r) => r.clone(),
        None => v,
    };
    Ok(serde_json::from_value(v)?)
}

/// The text `raw-diff` prints for two answers.
pub fn diff_text(a: &Raw, b: &Raw) -> String {
    let mut s = String::new();
    if a.id != b.id || a.datum != b.datum {
        s.push_str(&format!(
            "note: {} ({}) against {} ({})\n",
            a.id, a.datum, b.id, b.datum
        ));
    }
    s.push_str(&raw::render(&raw::diff(a, b)));
    s
}

/// `raw-diff <a> <b>`.
pub fn diff_files(w: &[String]) -> Result<()> {
    let [a, b] = w else {
        bail!("usage: h2 agent raw-diff <a.json> <b.json>");
    };
    print!("{}", diff_text(&load(a)?, &load(b)?));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn raw_becomes_world_raw() {
        let c = |s: &str| parse(&words(s)).unwrap().unwrap();
        assert_eq!(
            c("raw player.0"),
            ("world.raw".into(), json!({"id": "player.0"}))
        );
        assert_eq!(
            c("raw player.0 entity,shape 0x800"),
            (
                "world.raw".into(),
                json!({"id": "player.0", "parts": ["entity", "shape"], "len": 2048})
            )
        );
        assert!(parse(&words("object player.0")).is_none());
        assert!(parse(&words("raw")).unwrap().is_err());
        assert!(parse(&words("raw a b c d")).unwrap().is_err());
    }

    #[test]
    fn poke_becomes_world_poke() {
        let c = |s: &str| parse(&words(s)).unwrap();
        let expected = (
            "world.poke".to_owned(),
            json!({"id": "player.0", "part": "motion", "offset": 16, "hex": "ab"}),
        );
        assert_eq!(c("poke player.0 motion 0x10 ab").unwrap(), expected);
        assert_eq!(c("poke player.0 motion 16 ab").unwrap(), expected);
        assert!(c("poke player.0 motion -1 ab").is_err());
        assert!(c("poke player.0 motion 16").is_err());
        assert!(c("poke player.0 motion 16 ab extra").is_err());
    }

    #[test]
    fn diffs_two_saved_answers() {
        // As `h2 agent raw` prints one, and as a script line with the
        // response around it.
        let before = r#"{
  "at": {"tick": 100},
  "id": "player.0",
  "datum": "e0010001",
  "parts": [{"part": "shape", "base": "00455000", "hex": "0055450000000000"}]
}"#;
        let after = concat!(
            r#"{"at":{"tick":99}}"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":2,"result":{"id":"player.0","datum":"e0010001","#,
            r#""parts":[{"part":"shape","base":"00455000","hex":"005545000000003f"}]}}"#,
            "\n"
        );
        let a = parse_answer(before).unwrap();
        let b = parse_answer(after).unwrap();
        let text = diff_text(&a, &b);
        assert!(
            text.starts_with("shape  00455000 (0x8 bytes)  1 word differs\n"),
            "{text}"
        );
        assert!(
            text.contains("+0x004  00000000 -> 3f000000   0 -> 0.5"),
            "{text}"
        );
        assert!(parse_answer("not json").is_err());
    }
}
