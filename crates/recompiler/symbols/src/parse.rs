//! Reader for the XbSymbolDatabase OOVPA sources (`*_OOVPA.c` and `*.inl`).
//!
//! The database is C source built on preprocessor macros. This is not a C
//! parser: it understands exactly the handful of macro shapes the database
//! uses (signature headers, `OOVPA_SIG_MATCH` bodies, `REGISTER_OOVPAS*`
//! table entries, `#define` aliases and simple `#if` blocks).

use std::collections::HashMap;

/// One OOVPA: a set of `(offset, byte)` constraints plus address references.
#[derive(Debug, Clone, Default)]
pub struct Sig {
    pub xrefs: Vec<(u16, String)>,
    /// Sorted by offset.
    pub bytes: Vec<(u16, u8)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Function,
    /// Internal fragment (not an API function).
    Internal,
    Variable,
}

/// One `REGISTER_OOVPAS*` entry: a symbol and the revisions that detect it.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Name other signatures use to refer to this symbol's address.
    pub xref: String,
    pub name: String,
    pub kind: Kind,
    pub ltcg: bool,
    /// Registered only for the manual (hand-coded) scanner.
    pub manual: bool,
    /// Revision (XDK build) numbers in registration order, and the OOVPA
    /// variable name each resolves through.
    pub versions: Vec<u16>,
    pub var: String,
}

#[derive(Default, Debug)]
pub struct SigStore {
    /// `name_version` -> signature.
    pub sigs: HashMap<String, Sig>,
    /// `#define a b`: `a` is `b`.
    pub aliases: HashMap<String, String>,
}

impl SigStore {
    pub fn resolve(&self, var: &str, version: u16) -> Option<&Sig> {
        let mut key = format!("{var}_{version}");
        for _ in 0..8 {
            if let Some(s) = self.sigs.get(&key) {
                return Some(s);
            }
            key = self.aliases.get(&key)?.clone();
        }
        None
    }
}

/// Drop comments, join line continuations, evaluate `#if`/`#ifndef` blocks
/// the way a default build would (no `WIP_*` macros defined). `#define`
/// aliases and `#include` paths are collected on the way.
pub fn preprocess(src: &str, aliases: &mut HashMap<String, String>, includes: &mut Vec<String>) -> String {
    let src = src.replace("\\\r\n", " ").replace("\\\n", " ");
    // Block and line comments.
    let b = src.as_bytes();
    let mut nc = String::with_capacity(src.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
            nc.push(' ');
        } else if b[i] == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else {
            let ch_len = src[i..].chars().next().map_or(1, char::len_utf8);
            nc.push_str(&src[i..i + ch_len]);
            i += ch_len;
        }
    }
    // Conditionals: stack of (this branch live, parent live, any branch taken).
    let mut out = String::with_capacity(nc.len());
    let mut stack: Vec<(bool, bool, bool)> = Vec::new();
    let live = |s: &Vec<(bool, bool, bool)>| s.last().is_none_or(|t| t.0 && t.1);
    for line in nc.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix('#') {
            let rest = rest.trim_start();
            let (dir, arg) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            let arg = arg.trim();
            match dir {
                "if" | "ifdef" | "ifndef" => {
                    let parent = live(&stack);
                    let cond = match dir {
                        "if" => arg != "0",
                        "ifdef" => false,
                        _ => true, // no macro is defined
                    };
                    stack.push((cond, parent, cond));
                }
                "else" => {
                    if let Some(top) = stack.last_mut() {
                        top.0 = !top.2;
                        top.2 = true;
                    }
                }
                "endif" => {
                    stack.pop();
                }
                "define" if live(&stack) => {
                    let mut it = arg.split_whitespace();
                    if let (Some(a), Some(v), None) = (it.next(), it.next(), it.next())
                        && is_ident(a)
                        && is_ident(v)
                    {
                        aliases.insert(a.to_string(), v.to_string());
                    }
                }
                "include" if live(&stack) => {
                    if let Some(p) = arg.strip_prefix('"').and_then(|p| p.strip_suffix('"')) {
                        includes.push(p.to_string());
                    }
                }
                _ => {}
            }
            out.push('\n');
        } else {
            if live(&stack) {
                out.push_str(line);
            }
            out.push('\n');
        }
    }
    out
}

fn is_ident(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

fn is_ident_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// Index just past the `)` matching the `(` at `open`.
fn balanced(s: &str, open: usize) -> Option<usize> {
    let b = s.as_bytes();
    debug_assert_eq!(b[open], b'(');
    let mut depth = 0;
    for (i, &c) in b.iter().enumerate().skip(open) {
        match c {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// Split on top-level commas.
fn split_args(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut start) = (0i32, 0);
    for (i, c) in s.bytes().enumerate() {
        match c {
            b'(' | b'{' => depth += 1,
            b')' | b'}' => depth -= 1,
            b',' if depth == 0 => {
                out.push(s[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    let last = s[start..].trim();
    if !last.is_empty() {
        out.push(last);
    }
    out
}

fn num(s: &str) -> Option<u32> {
    let s = s.trim();
    match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(h) => u32::from_str_radix(h, 16).ok(),
        None => s.parse().ok(),
    }
}

/// Find each occurrence of `name(` at an identifier boundary.
fn calls<'a>(s: &'a str, name: &'a str) -> impl Iterator<Item = (usize, usize)> + 'a {
    let b = s.as_bytes();
    let mut from = 0;
    std::iter::from_fn(move || {
        while let Some(p) = s[from..].find(name) {
            let at = from + p;
            from = at + name.len();
            let before_ok = at == 0 || !is_ident_byte(b[at - 1]);
            let mut j = from;
            while j < b.len() && b[j] == b' ' {
                j += 1;
            }
            if before_ok && b.get(j) == Some(&b'(') {
                return Some((at, j));
            }
        }
        None
    })
}

fn off16(n: u32, what: &str) -> Result<u16, String> {
    u16::try_from(n).map_err(|_| format!("{what} offset {n:#x} does not fit 16 bits"))
}

fn byte(n: u32, what: &str) -> Result<u8, String> {
    u8::try_from(n).map_err(|_| format!("{what} value {n:#x} is not a byte"))
}

fn parse_body(body: &str, sig: &mut Sig) -> Result<(), String> {
    let b = body.as_bytes();
    let mut i = 0;
    while i < b.len() {
        // Slice only at ASCII matches: the body may hold any UTF-8.
        let at = &b[i..];
        if at.starts_with(b"XREF_ENTRY") || at.starts_with(b"OV_MATCH") || at.starts_with(b"OV_BYTE") {
            let rest = &body[i..];
            let open = rest.find('(').ok_or("macro without arguments")?;
            let end = balanced(rest, open).ok_or("unbalanced macro arguments")?;
            let args = split_args(&rest[open + 1..end - 1]);
            if rest.starts_with("XREF_ENTRY") {
                if let (Some(off), Some(name)) = (args.first().and_then(|a| num(a)), args.get(1)) {
                    sig.xrefs.push((off16(off, "XREF_ENTRY")?, name.trim_start_matches("XREF_").to_string()));
                }
            } else if let Some(off) = args.first().and_then(|a| num(a)) {
                for (k, a) in args[1..].iter().enumerate() {
                    if let Some(v) = num(a) {
                        let o = off
                            .checked_add(k as u32)
                            .ok_or_else(|| "OV_MATCH offset overflow".to_string())
                            .and_then(|o| off16(o, "OV_MATCH"))?;
                        sig.bytes.push((o, byte(v, "OV_MATCH")?));
                    }
                }
            }
            i += end;
        } else if b[i] == b'{' {
            let rest = &body[i..];
            if let Some(close) = rest.find('}') {
                let inner = &rest[1..close];
                if !inner.contains('{') {
                    let a = split_args(inner);
                    if a.len() == 2
                        && let (Some(off), Some(v)) = (num(a[0]), num(a[1]))
                    {
                        sig.bytes.push((off16(off, "{offset, value}")?, byte(v, "{offset, value}")?));
                        i += close + 1;
                        continue;
                    }
                }
            }
            i += 1;
        } else {
            i += 1;
        }
    }
    Ok(())
}

/// Parse every signature definition in one preprocessed `.inl` text.
pub fn parse_sigs(text: &str, store: &mut SigStore) -> Result<(), String> {
    const HEADERS: &[&str] = &[
        "OOVPA_SIG_HEADER_NO_XREF_DETECT",
        "OOVPA_SIG_HEADER_XREF_DETECT",
        "OOVPA_SIG_HEADER_NO_XREF",
        "OOVPA_SIG_HEADER_XREF",
        "OOVPA_NO_XREF_DETECT",
        "OOVPA_XREF_DETECT",
        "OOVPA_NO_XREF",
        "OOVPA_XREF",
    ];
    // (start of header, end of header call, name, version)
    let mut found: Vec<(usize, usize, String, u16)> = Vec::new();
    for h in HEADERS {
        for (at, open) in calls(text, h) {
            let Some(end) = balanced(text, open) else { continue };
            let args = split_args(&text[open + 1..end - 1]);
            if let (Some(n), Some(v)) = (args.first(), args.get(1).and_then(|a| num(a))) {
                let v = u16::try_from(v).map_err(|_| format!("signature {n}: revision {v} does not fit 16 bits"))?;
                found.push((at, end, n.to_string(), v));
            }
        }
    }
    found.sort();
    found.dedup_by_key(|f| f.0);
    for (k, (_, end, name, ver)) in found.iter().enumerate() {
        let next = found.get(k + 1).map_or(text.len(), |n| n.0);
        let tail = &text[*end..next];
        let t = tail.trim_start();
        let mut sig = Sig::default();
        if t.starts_with("OOVPA_SIG_MATCH_DUMMY") {
            continue;
        } else if t.starts_with("OOVPA_SIG_MATCH") {
            let off = tail.find("OOVPA_SIG_MATCH").unwrap();
            let Some(open) = tail[off..].find('(') else { continue };
            let open = off + open;
            let Some(close) = balanced(tail, open) else { continue };
            parse_body(&tail[open + 1..close - 1], &mut sig).map_err(|e| format!("signature {name}_{ver}: {e}"))?;
        } else {
            let stop = tail.find("OOVPA_END").unwrap_or(tail.len());
            parse_body(&tail[..stop], &mut sig).map_err(|e| format!("signature {name}_{ver}: {e}"))?;
        }
        if sig.bytes.is_empty() && sig.xrefs.is_empty() {
            continue;
        }
        sig.bytes.sort();
        sig.bytes.dedup();
        store.sigs.insert(format!("{name}_{ver}"), sig);
    }
    Ok(())
}

/// `REGISTER_OOVPAS_D3D(...)`-style helper macros: name -> (prefix, mode suffix).
fn prefix_macros(raw: &str) -> HashMap<String, (String, &'static str)> {
    let mut m = HashMap::new();
    for line in raw.lines() {
        let Some(rest) = line.trim().strip_prefix("#define") else { continue };
        let rest = rest.trim();
        let Some((name, body)) = rest.split_once('(') else { continue };
        if !name.starts_with("REGISTER_OOVPAS") {
            continue;
        }
        for (suffix, mode) in [("_PREFIX(", ""), ("_M_PREFIX(", "_M"), ("_C_PREFIX(", "_C")] {
            let needle = format!("REGISTER_OOVPAS{suffix}");
            if let Some(p) = body.find(&needle) {
                let after = &body[p + needle.len()..];
                if let Some(prefix) = after.split(',').next() {
                    m.insert(name.to_string(), (prefix.trim().to_string(), mode));
                }
            }
        }
    }
    m
}

/// Parse the `REGISTER_OOVPAS*` entries of a `*_OOVPA.c` file.
pub fn parse_table(raw: &str, text: &str) -> Result<Vec<Entry>, String> {
    let helpers = prefix_macros(raw);
    let mut starts: Vec<(usize, usize, String)> = Vec::new();
    let b = text.as_bytes();
    let mut from = 0;
    while let Some(p) = text[from..].find("REGISTER_OOVPAS") {
        let at = from + p;
        let mut j = at;
        while j < b.len() && is_ident_byte(b[j]) {
            j += 1;
        }
        from = j.max(at + 1);
        if at > 0 && is_ident_byte(b[at - 1]) {
            continue;
        }
        let mut k = j;
        while k < b.len() && b[k] == b' ' {
            k += 1;
        }
        if b.get(k) == Some(&b'(') {
            starts.push((at, k, text[at..j].to_string()));
        }
    }
    let mut out = Vec::new();
    for (_, open, name) in starts {
        let Some(end) = balanced(text, open) else { continue };
        let mut args = split_args(&text[open + 1..end - 1]);
        let (mut prefix, mut mode) = (None, "");
        let base = name.strip_prefix("REGISTER_OOVPAS").unwrap();
        match base {
            "" => {}
            "_M" => mode = "_M",
            "_C" => mode = "_C",
            "_PREFIX" | "_M_PREFIX" | "_C_PREFIX" => {
                if args.is_empty() {
                    continue;
                }
                prefix = Some(args.remove(0).to_string());
                mode = if base == "_M_PREFIX" {
                    "_M"
                } else if base == "_C_PREFIX" {
                    "_C"
                } else {
                    ""
                };
            }
            other => match helpers.get(&format!("REGISTER_OOVPAS{other}")) {
                Some((p, m)) => {
                    prefix = Some(p.clone());
                    mode = m;
                }
                None => continue,
            },
        }
        if args.len() < 2 {
            continue;
        }
        let (Some(open2), Some(close2)) = (args[0].find('('), args[0].rfind(')')) else { continue };
        if close2 < open2 {
            continue;
        }
        let sym = &args[0][..open2];
        let sargs = split_args(&args[0][open2 + 1..close2]);
        let Some(&name0) = sargs.first() else { continue };
        let sig_args = args[1];
        let mut versions: Vec<u16> = Vec::new();
        if let (Some(o), Some(c)) = (sig_args.find('('), sig_args.rfind(')'))
            && o < c
        {
            for v in split_args(&sig_args[o + 1..c]) {
                if let Some(n) = num(v) {
                    versions.push(u16::try_from(n).map_err(|_| format!("symbol {name0}: revision {n} does not fit 16 bits"))?);
                }
            }
        }
        let (kind, ltcg) = match sym {
            "SYM_INT" => (Kind::Internal, false),
            "SYM_VAR" => (Kind::Variable, false),
            "SYM_FUN_LTCG" => (Kind::Function, true),
            _ => (Kind::Function, false),
        };
        let mut var = name0.to_string();
        let stack = |a: &str| -> String {
            // STACK(n) -> "_n", STACK() -> ""
            let inner = a.trim().strip_prefix("STACK(").and_then(|r| r.strip_suffix(')')).unwrap_or("").trim();
            if inner.is_empty() { String::new() } else { format!("_{inner}") }
        };
        match sym {
            "SYM_FUN" if sargs.len() >= 4 => var += &stack(sargs[2]),
            "SYM_FUN_ALT" if sargs.len() >= 5 => {
                var += &stack(sargs[3]);
                var += sargs[1];
            }
            "SYM_FUN_LTCG" if sargs.len() >= 4 => {
                var += &stack(sargs[2]);
                var += "__LTCG";
                var += &ltcg_params(sargs[3]);
            }
            _ => {}
        }
        let xref = match &prefix {
            Some(p) => format!("{p}_{name0}"),
            None => name0.to_string(),
        };
        out.push(Entry { xref, name: name0.to_string(), kind, ltcg, manual: mode == "_M", versions, var });
    }
    Ok(out)
}

/// `PARAMS(PARAM(eax, a), PARAM(psh, b), PARAM(_dl, c))` -> `_eax1_dl3`.
fn ltcg_params(params: &str) -> String {
    let Some(open) = params.find('(') else { return String::new() };
    let inner = &params[open + 1..params.rfind(')').unwrap_or(params.len())];
    let mut s = String::new();
    for (i, p) in split_args(inner).iter().enumerate() {
        let reg = p
            .find('(')
            .map(|o| split_args(&p[o + 1..p.rfind(')').unwrap_or(p.len())]))
            .and_then(|a| a.first().map(|r| r.to_string()))
            .unwrap_or_default();
        if matches!(reg.as_str(), "psh" | "psh2" | "void" | "") {
            continue;
        }
        s += &format!("_{}{}", reg.trim_start_matches('_'), i + 1);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_and_aliases() {
        let src = r#"
// ***
OOVPA_SIG_HEADER_XREF(Foo_4, 4627, XRefOne)
OOVPA_SIG_MATCH(
    XREF_ENTRY(0x03, XREF_D3D_g_pDevice),
    OV_MATCH(0x00, 0x56, 0x8B), /* c */
    { 0x10, 0xC3 },
    //
);
#ifndef WIP_LessVertexPatching
OOVPA_NO_XREF(Old, 3911, 1)
    { { 0x00, 0x90 }, }
OOVPA_END;
#else
OOVPA_NO_XREF(Old, 3911, 2)
    { { 0x00, 0x91 }, }
OOVPA_END;
#endif
#define Foo_4_5028 Foo_4_4627
"#;
        let mut store = SigStore::default();
        let mut inc = Vec::new();
        let t = preprocess(src, &mut store.aliases, &mut inc);
        parse_sigs(&t, &mut store).unwrap();
        let foo = store.resolve("Foo_4", 4627).unwrap();
        assert_eq!(foo.bytes, vec![(0, 0x56), (1, 0x8B), (0x10, 0xC3)]);
        assert_eq!(foo.xrefs, vec![(3, "D3D_g_pDevice".to_string())]);
        assert_eq!(store.resolve("Old", 3911).unwrap().bytes, vec![(0, 0x90)]);
        assert_eq!(store.resolve("Foo_4", 5028).unwrap().bytes.len(), 3);
    }

    fn sigs_of(src: &str) -> Result<SigStore, String> {
        let mut store = SigStore::default();
        let t = preprocess(src, &mut store.aliases, &mut Vec::new());
        parse_sigs(&t, &mut store)?;
        Ok(store)
    }

    #[test]
    fn non_ascii_body_does_not_panic() {
        let s = sigs_of("OOVPA_SIG_HEADER_NO_XREF(F, 1)\nOOVPA_SIG_MATCH(\n // caf\u{e9} \u{1f600}\n { 0x00, 0x90 }, \u{e9} OV_MATCH(0x01, 0xC3)\n);").unwrap();
        assert_eq!(s.resolve("F", 1).unwrap().bytes, vec![(0, 0x90), (1, 0xC3)]);
        // Non-ASCII directly in the body, not just in comments.
        let s = sigs_of("OOVPA_SIG_HEADER_NO_XREF(G, 1)\nOOVPA_SIG_MATCH(\u{e9}{ 0x00, 0x90 }\u{e9})\n;").unwrap();
        assert_eq!(s.resolve("G", 1).unwrap().bytes, vec![(0, 0x90)]);
    }

    #[test]
    fn out_of_range_numbers_are_errors() {
        let e = sigs_of("OOVPA_SIG_HEADER_NO_XREF(F, 1)\nOOVPA_SIG_MATCH(OV_MATCH(65535, 0x90, 0xc3));").unwrap_err();
        assert!(e.contains("F_1") && e.contains("OV_MATCH"), "{e}");
        assert!(sigs_of("OOVPA_SIG_HEADER_NO_XREF(F, 1)\nOOVPA_SIG_MATCH({ 0x10000, 0x90 });").is_err());
        assert!(sigs_of("OOVPA_SIG_HEADER_NO_XREF(F, 1)\nOOVPA_SIG_MATCH({ 0x10, 0x190 });").is_err());
        assert!(sigs_of("OOVPA_SIG_HEADER_NO_XREF(F, 70000)\nOOVPA_SIG_MATCH({ 0x10, 0x90 });").is_err());
        assert!(sigs_of("OOVPA_SIG_HEADER_NO_XREF(F, 1)\nOOVPA_SIG_MATCH(OV_MATCH(4294967295, 1, 2));").is_err());
        assert!(sigs_of("OOVPA_SIG_HEADER_NO_XREF(F, 1)\nOOVPA_SIG_MATCH(OV_MATCH(65534, 0x90, 0xc3));").is_ok());
    }

    #[test]
    fn table_entries() {
        let raw = "#define REGISTER_OOVPAS_D3D(Symbol, ...)  REGISTER_OOVPAS_PREFIX(D3D, Symbol, __VA_ARGS__)";
        let body = r#"
REGISTER_OOVPAS(SYM_FUN(D3DDevice_Clear, CALL(std), STACK(24), PARAMS(PARAM(psh, A))),
                SYM_SIG(3911, 4034)),
REGISTER_OOVPAS_D3D(SYM_FUN_LTCG(CDevice_Init, CALL(unk), STACK(4), PARAMS(PARAM(esi, this), PARAM(psh, p), PARAM(_dl, q))),
                    SYM_SIG(5849)),
REGISTER_OOVPAS_M(SYM_FUN(Hand, CALL(std), STACK(), PARAMS(PARAM1(void))), SYM_SIG(3911)),
"#;
        let mut a = HashMap::new();
        let text = preprocess(&format!("{raw}\n{body}"), &mut a, &mut Vec::new());
        let e = parse_table(raw, &text).unwrap();
        assert_eq!(e.len(), 3);
        assert_eq!(e[0].var, "D3DDevice_Clear_24");
        assert_eq!(e[0].versions, vec![3911, 4034]);
        assert_eq!(e[1].var, "CDevice_Init_4__LTCG_esi1_dl3");
        assert_eq!(e[1].xref, "D3D_CDevice_Init");
        assert!(e[1].ltcg);
        assert!(e[2].manual);
        assert_eq!(e[2].var, "Hand");
    }
}
