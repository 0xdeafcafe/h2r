//! Object manager names use backslashes. No host path interpretation occurs here.
use crate::{Error, FATX_MAX_NAME, FATX_MAX_PATH, Result};
use std::collections::BTreeMap;

pub(crate) fn object_name(name: &str) -> Result<String> {
    let bytes = name.as_bytes();
    let expanded = if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        if bytes.len() > 2 && bytes[2] != b'\\' {
            return Err(Error::ObjectNameInvalid);
        }
        format!("\\??\\{name}")
    } else {
        name.to_owned()
    };
    if !expanded.starts_with('\\')
        || expanded.contains(['/', '\0'])
        || !expanded.is_ascii()
        || expanded.bytes().any(|b| b < 0x20 || b == 0x7f)
    {
        return Err(Error::ObjectNameInvalid);
    }
    let normalized = expanded.trim_end_matches('\\');
    if normalized.is_empty()
        || normalized[1..]
            .split('\\')
            .any(|p| p.is_empty() || p == "." || p == "..")
    {
        return Err(Error::ObjectNameInvalid);
    }
    Ok(normalized.to_owned())
}

pub(crate) fn prefix<'a>(path: &'a str, base: &str) -> Option<&'a str> {
    if path.len() >= base.len()
        && path[..base.len()].eq_ignore_ascii_case(base)
        && (path.len() == base.len() || path.as_bytes()[base.len()] == b'\\')
    {
        Some(path[base.len()..].trim_start_matches('\\'))
    } else {
        None
    }
}

pub(crate) fn components(path: &str) -> Result<Vec<&str>> {
    if path.is_empty() {
        return Ok(Vec::new());
    }
    let parts: Vec<_> = path.split('\\').collect();
    if parts.iter().any(|p| {
        p.is_empty()
            || *p == "."
            || *p == ".."
            || p.bytes()
                .any(|b| !(0x20..0x7f).contains(&b) || b"\\/:*?\"<>|".contains(&b))
    }) {
        return Err(Error::ObjectNameInvalid);
    }
    Ok(parts)
}

pub fn validate_fatx_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > FATX_MAX_NAME
        || name == "."
        || name == ".."
        || name
            .bytes()
            .any(|b| !(0x20..0x7f).contains(&b) || b"\"*+,/:;<=>?\\|".contains(&b))
    {
        return Err(Error::ObjectNameInvalid);
    }
    Ok(())
}
pub fn validate_fatx_path(path: &str) -> Result<()> {
    if path.len() > FATX_MAX_PATH {
        return Err(Error::ObjectNameInvalid);
    }
    for part in components(path)? {
        validate_fatx_name(part)?;
    }
    Ok(())
}

/// Case-insensitive object-manager aliases, with longest component-prefix resolution.
#[derive(Debug, Default)]
pub struct Namespace {
    links: BTreeMap<String, String>,
}
impl Namespace {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn create_symbolic_link(&mut self, name: &str, target: &str) -> Result<()> {
        let name = object_name(name)?.to_ascii_lowercase();
        let target = object_name(target)?;
        if self.links.contains_key(&name) {
            return Err(Error::ObjectNameCollision);
        }
        self.links.insert(name, target);
        Ok(())
    }
    pub fn delete_symbolic_link(&mut self, name: &str) -> Result<()> {
        self.links
            .remove(&object_name(name)?.to_ascii_lowercase())
            .map(|_| ())
            .ok_or(Error::ObjectNameNotFound)
    }
    pub fn query_symbolic_link(&self, name: &str) -> Result<&str> {
        self.links
            .get(&object_name(name)?.to_ascii_lowercase())
            .map(String::as_str)
            .ok_or(Error::ObjectNameNotFound)
    }
    pub fn resolve(&self, name: &str) -> Result<String> {
        let mut path = object_name(name)?;
        for _ in 0..32 {
            let matched = self
                .links
                .iter()
                .filter_map(|(key, target)| {
                    prefix(&path, key).map(|rest| (key.len(), target, rest))
                })
                .max_by_key(|(len, _, _)| *len);
            let Some((_, target, rest)) = matched else {
                return Ok(path);
            };
            path = if rest.is_empty() {
                target.clone()
            } else {
                format!("{target}\\{rest}")
            };
            if path.len() > 32768 {
                return Err(Error::ReparsePointNotResolved);
            }
        }
        Err(Error::ReparsePointNotResolved)
    }
}

/// Simple NT-style `*` / `?` matching, ASCII insensitive; `*.*` also matches extensionless names.
/// DOS_STAR, DOS_QM and DOS_DOT expressions are deliberately not accepted.
pub fn wildcard_matches(pattern: &str, name: &str) -> bool {
    let pattern = if pattern == "*.*" { "*" } else { pattern };
    let (p, n) = (pattern.as_bytes(), name.as_bytes());
    let (mut i, mut j, mut star, mut retry) = (0, 0, None, 0);
    while j < n.len() {
        if i < p.len() && (p[i] == b'?' || p[i].eq_ignore_ascii_case(&n[j])) {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == b'*' {
            star = Some(i);
            i += 1;
            retry = j;
        } else if let Some(s) = star {
            i = s + 1;
            retry += 1;
            j = retry;
        } else {
            return false;
        }
    }
    while i < p.len() && p[i] == b'*' {
        i += 1;
    }
    i == p.len()
}
