//! Just enough XML to read nextest's JUnit report: elements, attributes,
//! text, CDATA and the five named entities plus numeric ones. No
//! namespaces, no DTDs.

use anyhow::{Result, bail};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Element>,
    pub text: String,
}

impl Element {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// Every element below this one called `name`, depth first.
    pub fn descendants_named<'a>(&'a self, name: &str, out: &mut Vec<&'a Element>) {
        for c in &self.children {
            if c.name == name {
                out.push(c);
            }
            c.descendants_named(name, out);
        }
    }
}

/// The document's root element.
pub fn parse(xml: &str) -> Result<Element> {
    let mut stack = vec![Element { name: "#document".into(), ..Element::default() }];
    let mut i = 0;
    while i < xml.len() {
        let rest = &xml[i..];
        if let Some(r) = rest.strip_prefix("<?") {
            i += 2 + r.find("?>").map_or(r.len(), |e| e + 2);
        } else if let Some(r) = rest.strip_prefix("<!--") {
            i += 4 + r.find("-->").map_or(r.len(), |e| e + 3);
        } else if let Some(r) = rest.strip_prefix("<![CDATA[") {
            let Some(e) = r.find("]]>") else { bail!("unterminated CDATA") };
            stack.last_mut().expect("never empty").text.push_str(&r[..e]);
            i += 9 + e + 3;
        } else if let Some(r) = rest.strip_prefix("<!") {
            i += 2 + r.find('>').map_or(r.len(), |e| e + 1);
        } else if let Some(r) = rest.strip_prefix("</") {
            let Some(e) = r.find('>') else { bail!("unterminated end tag") };
            let name = r[..e].trim();
            if stack.len() < 2 {
                bail!("</{name}> closes nothing");
            }
            let el = stack.pop().expect("checked");
            if el.name != name {
                bail!("</{name}> closes <{}>", el.name);
            }
            stack.last_mut().expect("checked").children.push(el);
            i += 2 + e + 1;
        } else if let Some(r) = rest.strip_prefix('<') {
            let (el, used, closed) = start_tag(r)?;
            i += 1 + used;
            if closed {
                stack.last_mut().expect("never empty").children.push(el);
            } else {
                stack.push(el);
            }
        } else {
            let e = rest.find('<').unwrap_or(rest.len());
            stack.last_mut().expect("never empty").text.push_str(&unescape(&rest[..e]));
            i += e;
        }
    }
    if stack.len() != 1 {
        bail!("<{}> is never closed", stack.last().expect("never empty").name);
    }
    let doc = stack.pop().expect("one left");
    match doc.children.into_iter().next() {
        Some(root) => Ok(root),
        None => bail!("no root element"),
    }
}

/// A start tag after its `<`: (element, bytes used, self-closing).
fn start_tag(s: &str) -> Result<(Element, usize, bool)> {
    let name_end = s.find(|c: char| c.is_whitespace() || c == '/' || c == '>').unwrap_or(s.len());
    let mut el = Element { name: s[..name_end].to_owned(), ..Element::default() };
    let mut i = name_end;
    loop {
        let rest = &s[i..];
        let trimmed = rest.trim_start();
        i += rest.len() - trimmed.len();
        if trimmed.starts_with("/>") {
            return Ok((el, i + 2, true));
        }
        if trimmed.starts_with('>') {
            return Ok((el, i + 1, false));
        }
        let Some(eq) = trimmed.find('=') else { bail!("bad attribute in <{}>", el.name) };
        let key = trimmed[..eq].trim().to_owned();
        let after = trimmed[eq + 1..].trim_start();
        let skipped = trimmed.len() - after.len();
        let Some(quote) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else {
            bail!("unquoted attribute {key} in <{}>", el.name)
        };
        let Some(close) = after[1..].find(quote) else { bail!("unterminated attribute {key} in <{}>", el.name) };
        el.attrs.push((key, unescape(&after[1..1 + close])));
        i += skipped + 1 + close + 1;
    }
}

fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(semi) = rest.find(';').filter(|n| *n <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..semi];
        let ch = match entity {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            e if e.starts_with("#x") || e.starts_with("#X") => u32::from_str_radix(&e[2..], 16).ok().and_then(char::from_u32),
            e if e.starts_with('#') => e[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_elements_attributes_and_text() {
        let doc = parse(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!-- a comment -->
<a x="1" y='two &amp; three'>
  <b/>
  <c z="&lt;&#65;&#x42;&gt;">hi &quot;there&quot;<![CDATA[ <raw> & ]]></c>
</a>"#,
        )
        .unwrap();
        assert_eq!(doc.name, "a");
        assert_eq!(doc.attr("x"), Some("1"));
        assert_eq!(doc.attr("y"), Some("two & three"));
        assert_eq!(doc.children.len(), 2);
        assert_eq!(doc.children[0].name, "b");
        let c = &doc.children[1];
        assert_eq!(c.attr("z"), Some("<AB>"));
        assert_eq!(c.text, "hi \"there\" <raw> & ");
        let mut found = Vec::new();
        doc.descendants_named("c", &mut found);
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn refuses_broken_documents() {
        assert!(parse("<a><b></a>").is_err());
        assert!(parse("<a>").is_err());
        assert!(parse("text only").is_err());
    }

    #[test]
    fn keeps_unknown_entities_and_bare_ampersands() {
        assert_eq!(unescape("a & b &nbsp; &#;"), "a & b &nbsp; &#;");
    }
}
